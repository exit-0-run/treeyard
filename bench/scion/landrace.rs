//! Landing load test in the shape of TreeYard's landbench (gitai's graft-bench): `--agents` agents
//! each land `--changes` changes on `main`. Every change adds its own file; a share of them
//! (`--conflict-rate`, same seeded choice as landbench) also appends a line at the end of
//! `shared.txt`, so two such changes made from the same base conflict. No conflict resolver: a
//! change that gets MERGE_CONFLICT fails, as in landbench. Each agent has its own client.
//!
//! One change: ResolveRef main, (ReadPath shared.txt), UploadObjects, CreateChangeset on the tip,
//! Land main. Land rebases on the server and retries the ref CAS itself (SPEC 8.4).
//!
//!   landrace [--backend memory|external] [--agents 50] [--changes 5] [--conflict-rate 0.2] [--seed 1]
//!
//! `external` uses CockroachDB + S3 from the SCION_TEST_* variables (scripts/test-external.sh).
//! Prints one JSON object with the landbench fields.
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use scion_conformance::{Backend, ClusterOpts, TestCluster, Via};
use scion_core::{ErrorCode, ObjectId};
use scion_proto::{ScionClient, error_from_status, v1};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Barrier;

struct Opts {
    agents: usize,
    changes: usize,
    conflict_rate: f64,
    seed: u64,
}

/// Same function as landbench's `shared`, so both tests pick the same changes.
fn shared(o: &Opts, i: usize, j: usize) -> bool {
    let mut x = o.seed ^ ((i as u64) << 32) ^ j as u64 ^ 0x9e37_79b9_7f4a_7c15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x % 10_000) as f64 / 10_000.0 < o.conflict_rate
}

#[derive(Default)]
struct Sample {
    landed: bool,
    conflict: bool,
    ms: f64,
    error: Option<String>,
}

fn id_of(b: &[u8]) -> Result<ObjectId> {
    Ok(ObjectId::from_bytes(b)?)
}

async fn tip(c: &ScionClient) -> Result<ObjectId> {
    let r = c
        .objects()
        .resolve_ref(v1::ResolveRefRequest { name: "main".into(), at_version: None })
        .await
        .map_err(|s| error_from_status(&s))?
        .into_inner();
    id_of(&r.target)
}

/// Retries UNAVAILABLE, as landbench retries git's transient fetch failures: under 50 writers the
/// CockroachDB backend returns "transaction retries exhausted" now and then, and an agent would call
/// again. Every other error goes back to the caller.
async fn retry<T, F, Fut>(retries: &mut usize, mut f: F) -> Result<T, scion_core::ScionError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, tonic::Status>>,
{
    let mut k = 0u64;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(st) => {
                let e = error_from_status(&st);
                if e.code != ErrorCode::Unavailable || k >= 30 {
                    return Err(e);
                }
                *retries += 1;
                k += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20 * k)).await;
            }
        }
    }
}

async fn one(o: &Opts, i: usize, j: usize, c: &ScionClient, retries: &mut usize) -> Result<(), scion_core::ScionError> {
    let r = retry(retries, || async {
        c.objects().resolve_ref(v1::ResolveRefRequest { name: "main".into(), at_version: None }).await
    })
    .await?
    .into_inner();
    let base = ObjectId::from_bytes(&r.target)?;
    let mut puts: Vec<(String, Vec<u8>)> =
        vec![(format!("agents/a{i:03}/c{j:02}.txt"), format!("agent {i} change {j}\n").into_bytes())];
    if shared(o, i, j) {
        let mut text = Vec::new();
        let mut stream = retry(retries, || async {
            c.query()
                .read_path(v1::ReadPathRequest { changeset: base.as_bytes().to_vec(), path: b"shared.txt".to_vec() })
                .await
        })
        .await?
        .into_inner();
        while let Some(chunk) = stream.next().await {
            text.extend(chunk.map_err(|s| error_from_status(&s))?.data);
        }
        text.extend(format!("agent {i} change {j}\n").into_bytes());
        puts.push(("shared.txt".into(), text));
    }
    let items: Vec<v1::UploadItem> = puts
        .iter()
        .map(|(_, b)| v1::UploadItem {
            id: ObjectId::hash_blob(b).as_bytes().to_vec(),
            kind: u32::from(scion_core::Kind::Blob.byte()),
            data: b.clone(),
        })
        .collect();
    retry(retries, || async { c.write().upload_objects(futures::stream::iter(items.clone())).await }).await?;
    let refs: Vec<(&str, &[u8])> = puts.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
    let req = TestCluster::create_request(&base, &refs, &[], (i * 1000 + j) as u32);
    let head = retry(retries, || async { c.write().create_changeset(req.clone()).await }).await?.into_inner();
    retry(retries, || async {
        c.write()
            .land(v1::LandRequest { r#ref: "main".into(), head: head.changeset_id.clone(), allow_empty: false })
            .await
    })
    .await?;
    Ok(())
}

async fn agent(o: Arc<Opts>, i: usize, c: ScionClient) -> (Vec<Sample>, usize) {
    let (mut out, mut retries) = (Vec::new(), 0);
    for j in 0..o.changes {
        let t0 = Instant::now();
        let mut s = Sample::default();
        match one(&o, i, j, &c, &mut retries).await {
            Ok(()) => s.landed = true,
            Err(e) if e.code == ErrorCode::MergeConflict => {
                s.conflict = true;
                s.error = Some("merge conflict".into());
            }
            Err(e) => s.error = Some(e.code.to_string()),
        }
        s.ms = t0.elapsed().as_secs_f64() * 1000.0;
        out.push(s);
    }
    (out, retries)
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let k = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    (sorted[k.clamp(1, sorted.len()) - 1] * 10.0).round() / 10.0
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|p| args.get(p + 1)).cloned();
    let backend = match get("--backend").as_deref().unwrap_or("memory") {
        "memory" => Backend::Memory,
        "external" if Backend::external_available() => Backend::External,
        "external" => bail!("external: set SCION_TEST_CRDB_URL and SCION_TEST_S3_* (scripts/test-external.sh)"),
        b => bail!("unknown backend {b}"),
    };
    let o = Arc::new(Opts {
        agents: get("--agents").map_or(Ok(50), |v| v.parse())?,
        changes: get("--changes").map_or(Ok(5), |v| v.parse())?,
        conflict_rate: get("--conflict-rate").map_or(Ok(0.2), |v| v.parse())?,
        seed: get("--seed").map_or(Ok(1), |v| v.parse())?,
    });
    let label = if matches!(backend, Backend::Memory) { "scion-memory" } else { "scion-external" };
    let cluster = TestCluster::start(ClusterOpts::new(backend).with_daemon(false)).await;
    let seeder = TestCluster::seeder();
    let admin = cluster.client(Via::Server, &seeder).await;
    // Seed: shared.txt only, as bench/landbench.sh's seed repository.
    let seed: Vec<(&str, &[u8])> = vec![("shared.txt", b"shared file\n")];
    let blobs: Vec<&[u8]> = seed.iter().map(|(_, b)| *b).collect();
    TestCluster::upload(&admin, &blobs).await;
    // The test cluster starts with `main`; put the seed on top of it.
    let req = TestCluster::create_request(&tip(&admin).await?, &seed, &[], 999_999);
    let root = admin.write().create_changeset(req).await?.into_inner();
    admin
        .write()
        .land(v1::LandRequest { r#ref: "main".into(), head: root.changeset_id, allow_empty: false })
        .await?;
    let mut clients = Vec::new();
    for _ in 0..o.agents {
        clients.push(cluster.client(Via::Server, &seeder).await);
    }
    let start = Arc::new(Barrier::new(o.agents + 1));
    let mut hs = Vec::new();
    for (i, c) in clients.into_iter().enumerate() {
        let (o, start) = (o.clone(), start.clone());
        hs.push(tokio::spawn(async move {
            start.wait().await;
            agent(o, i, c).await
        }));
    }
    start.wait().await;
    let t0 = Instant::now();
    let (mut samples, mut retries) = (Vec::new(), 0);
    for h in hs {
        let (v, r) = h.await.context("join")?;
        samples.extend(v);
        retries += r;
    }
    let wall = t0.elapsed().as_secs_f64();
    let landed = samples.iter().filter(|s| s.landed).count();
    let conflicts = samples.iter().filter(|s| s.conflict).count();
    let mut lat: Vec<f64> = samples.iter().filter(|s| s.landed).map(|s| s.ms).collect();
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    for s in &samples {
        if let Some(e) = &s.error {
            *kinds.entry(e.clone()).or_default() += 1;
        }
    }
    println!(
        "{}",
        json!({
            "mode": label,
            "agents": o.agents,
            "changes": o.agents * o.changes,
            "landed": landed,
            "failed": samples.len() - landed,
            "conflicts": conflicts,
            "attempts_per_land": 1.0,
            "duration_seconds": (wall * 100.0).round() / 100.0,
            "lands_per_minute": (landed as f64 / wall * 60.0).round(),
            "p50_ms": pct(&lat, 50.0),
            "p95_ms": pct(&lat, 95.0),
            "p99_ms": pct(&lat, 99.0),
            "failure_kinds": kinds,
            "unavailable_retries": retries,
        })
    );
    Ok(())
}
