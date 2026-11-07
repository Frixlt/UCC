//! Power limits command implementation.

use serde_json::Value;
use crate::client::CliClient;
use crate::format::*;

pub async fn run_power_limits(client: &CliClient, json_mode: bool) -> Result<(), i32> {
    let raw = client.proxy().odm_power_limits_json().await.unwrap_or_else(|_| "[]".to_string());
    let val: Value = serde_json::from_str(&raw).unwrap_or(Value::Array(vec![]));

    let limits: Vec<i64> = val.as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();

    if json_mode {
        print_json(&val);
        return Ok(());
    }

    if limits.is_empty() {
        println!("No ODM power limits available");
        return Ok(());
    }

    println!("ODM Power Limits:");
    for (i, limit) in limits.iter().enumerate() {
        println!("  {:<24} {} W", tdp_label(i), limit);
    }

    Ok(())
}
