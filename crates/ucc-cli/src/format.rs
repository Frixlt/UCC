//! Output formatting helpers for UCC CLI matching C++ baseline.

use serde_json::Value;

/// Pretty-print or dump a JSON value or string.
pub fn print_json(val: &Value) {
    if let Ok(pretty) = serde_json::to_string_pretty(val) {
        println!("{}", pretty);
    } else {
        println!("{}", val);
    }
}

/// Print formatted key-value pair with unit.
pub fn print_val_opt<T: std::fmt::Display>(label: &str, v: Option<T>, unit: &str) {
    if let Some(val) = v {
        if unit.is_empty() {
            println!("  {:<24} {}", label, val);
        } else {
            println!("  {:<24} {} {}", label, val, unit);
        }
    } else {
        println!("  {:<24} n/a", label);
    }
}

/// Print formatted key-value pair with float and precision.
pub fn print_val_f64(label: &str, v: Option<f64>, unit: &str) {
    if let Some(val) = v {
        if unit.is_empty() {
            println!("  {:<24} {:.1}", label, val);
        } else {
            println!("  {:<24} {:.1} {}", label, val, unit);
        }
    } else {
        println!("  {:<24} n/a", label);
    }
}

/// Print formatted boolean yes/no.
pub fn print_val_bool(label: &str, v: Option<bool>) {
    if let Some(val) = v {
        println!("  {:<24} {}", label, if val { "yes" } else { "no" });
    } else {
        println!("  {:<24} n/a", label);
    }
}

/// Print OK on success, or FAILED to stderr on failure.
pub fn ok(success: bool) -> Result<(), i32> {
    if success {
        println!("OK");
        Ok(())
    } else {
        eprintln!("FAILED");
        Err(1)
    }
}

/// Human-readable TDP label for index.
pub fn tdp_label(idx: usize) -> &'static str {
    match idx {
        0 => "PL1 (Sustained):",
        1 => "PL2 (Boost):",
        2 => "PL4 (Peak):",
        _ => "Unknown:",
    }
}

/// Human-readable power state label.
pub fn power_state_label(raw: &str) -> String {
    match raw {
        "power_type_c" => "Type-C".to_string(),
        "power_ac" => "AC".to_string(),
        "power_bat" => "Battery".to_string(),
        "power_wc" => "AC w/ Water Cooler".to_string(),
        other => other.to_string(),
    }
}

/// Convert power state to short uppercase label (e.g. "power_ac" -> "AC").
pub fn state_short_label(state: &str) -> String {
    let s = state.strip_prefix("power_").unwrap_or(state);
    s.to_uppercase()
}

/// Build an annotation string like " [AC, WC]" from a list of power state keys.
pub fn assignment_tag(states: &[String]) -> String {
    if states.is_empty() {
        return String::new();
    }
    let labels: Vec<String> = states.iter().map(|s| state_short_label(s)).collect();
    format!(" [{}]", labels.join(", "))
}

/// Extract human-readable name from profile JSON object.
pub fn profile_name(obj: &Value) -> String {
    obj.get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("(unknown)")
        .to_string()
}

/// Extract id from profile JSON object.
pub fn profile_id(obj: &Value) -> String {
    obj.get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Parse boolean string ("on"/"off", "true"/"false", "1"/"0").
pub fn parse_bool_arg(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "on" | "true" | "1" | "yes" | "enable" => Some(true),
        "off" | "false" | "0" | "no" | "disable" => Some(false),
        _ => None,
    }
}
