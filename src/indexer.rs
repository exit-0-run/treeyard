//! Eager derived data (`[index] search = "eager"` or `symbols = "eager"`): a background
//! thread that follows main and brings the history index, the trigram index and the symbol
//! index up to date after every move. Heavy work runs in chunks outside the repo lock.

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;

use crate::config::IndexMode;
use crate::repo::Repo;
use crate::{search, symbols};

/// One catch-up pass for `main`; returns (trigram rows, symbol rows) it computed.
pub fn catch_up(repo: &Repo, search_on: bool, symbols_on: bool) -> Result<(usize, usize)> {
    let main = repo.with(|r| {
        r.sync_history()?;
        r.main()
    })?;
    let mut done = (0, 0);
    if search_on {
        done.0 = search::prepare(repo, &main)?;
        search::build_index(repo, &main)?;
        repo.with(|r| r.search_index(&main))?;
    }
    if symbols_on {
        done.1 = symbols::prepare(repo, &main)?;
    }
    Ok(done)
}

/// Starts the thread when some index is eager.
pub fn spawn(repo: Arc<Repo>) -> Option<std::thread::JoinHandle<()>> {
    let (s, y) = repo.with(|r| Ok((r.cfg.index.search, r.cfg.index.symbols))).ok()?;
    let (search_on, symbols_on) = (s == IndexMode::Eager, y == IndexMode::Eager);
    if !search_on && !symbols_on {
        return None;
    }
    Some(std::thread::spawn(move || {
        let mut last = String::new();
        loop {
            let main = repo.with(|r| r.main()).unwrap_or_default();
            if main != last {
                match catch_up(&repo, search_on, symbols_on) {
                    Ok(_) => last = main,
                    Err(e) => eprintln!("treeyard indexer: {e:#}"),
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }))
}
