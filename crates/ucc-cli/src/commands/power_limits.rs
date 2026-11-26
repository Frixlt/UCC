//! Power limits command implementation.

use serde_json::Value;
use crate::client::{with_rpc_timeout, CliClient};
use crate::format::*;

pub async fn run_power_limits(client: &CliClient, json_mode: bool) -> Result<(), i32> {
    let raw = with_rpc_timeout(client.proxy().odm_power_limits_json())
        .await
        .map_err(|_| {
            eprintln!("Error: Could not retrieve ODM power limits");
            1
        })?;
    let val: Value = serde_json::from_str(&raw).unwrap_or(Value::Array(vec![]));

    if json_mode {
        print_json(&val);
        return Ok(());
    }

    let entries: Vec<(usize, i64, Option<i64>, Option<i64>)> = val
        .as_array()
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(pos, v)| {
                    if let Some(w) = v.as_i64() {
                        Some((pos, w, None, None))
                    } else if let Some(obj) = v.as_object() {
                        let cur = obj.get("current").and_then(|c| c.as_i64())?;
                        let idx = obj
                            .get("index")
                            .and_then(|i| i.as_u64())
                            .map(|i| i as usize)
                            .unwrap_or(pos);
                        let min = obj.get("min").and_then(|m| m.as_i64());
                        let max = obj.get("max").and_then(|m| m.as_i64());
                        Some((idx, cur, min, max))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    if entries.is_empty() {
        println!("No ODM power limits available");
        return Ok(());
    }

    println!("ODM Power Limits:");
    for (idx, current, min, max) in entries {
        match (min, max) {
            (Some(mn), Some(mx)) => println!("  {:<24} {} W ({}-{} W)", tdp_label(idx), current, mn, mx),
            _ => println!("  {:<24} {} W", tdp_label(idx), current),
        }
    }

    Ok(())
}
