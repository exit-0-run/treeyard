//! Blocking HTTP client for the API, used by the CLI and the MCP bridge.

use anyhow::{Result, bail};
use serde_json::Value;

pub fn call(url: &str, op: &str, params: Value) -> Result<Value> {
    let endpoint = format!("{}/api/{op}", url.trim_end_matches('/'));
    match ureq::post(&endpoint).send_json(params) {
        Ok(resp) => Ok(resp.into_json()?),
        Err(ureq::Error::Status(code, resp)) => {
            let body: Value = resp.into_json().unwrap_or(Value::Null);
            match body["error"].as_str() {
                Some(msg) => bail!("{msg}"),
                None => bail!("HTTP {code} from {endpoint}"),
            }
        }
        Err(e) => bail!("cannot reach rootstock at {url}: {e}"),
    }
}
