//! Semantic checker for JSON artifacts produced by the mem0 impulse lane.
//!
//! Usage: `mem0_json_check backend=path.json [...]`. Every document is checked
//! independently; when several backends are supplied their FIR-level
//! `compute_cost` values must be identical.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::process::ExitCode;

use serde_json::Value;

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mem0-json-check: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let mut seen = 0usize;
    let mut common_cost = None;
    let mut backend_layouts = HashMap::<String, Value>::new();
    for argument in args {
        let (backend, path) = argument
            .split_once('=')
            .ok_or_else(|| format!("expected backend=path, got `{argument}`"))?;
        let text =
            fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("invalid JSON in {path}: {error}"))?;
        validate_document(backend, &value)?;
        match backend_layouts.get(backend) {
            Some(expected) if expected != &value["memory_layout"] => {
                return Err(format!(
                    "{backend} memory_layout differs between supplied documents"
                ));
            }
            None => {
                backend_layouts.insert(backend.to_owned(), value["memory_layout"].clone());
            }
            _ => {}
        }
        match &common_cost {
            Some(expected) if expected != &value["compute_cost"] => {
                return Err(format!(
                    "{backend} compute_cost differs from the other backends"
                ));
            }
            None => common_cost = Some(value["compute_cost"].clone()),
            _ => {}
        }
        seen += 1;
    }
    if seen == 0 {
        return Err("at least one backend=path argument is required".to_owned());
    }
    Ok(())
}

fn validate_document(backend: &str, value: &Value) -> Result<(), String> {
    let expected_manager = if backend == "cpp" {
        "dsp_memory_manager_v1"
    } else {
        "faust_memory_manager_v1"
    };
    if value["memory_layout_version"] != 2
        || value["memory_manager"]["mode"] != "mem0"
        || value["memory_manager"]["backend"] != backend
        || value["memory_manager"]["manager_abi"] != expected_manager
        || value["compute_cost_version"] != 2
        || value["compute_cost_metric"] != "static_scalar_fir_structure"
    {
        return Err(format!("{backend} has an invalid mem0 schema header"));
    }
    if backend == "cranelift" && value["compute_body_lowered"] != true {
        return Err("Cranelift impulse JSON exposes a fallback compute body".to_owned());
    }
    let zones = value["memory_layout"]
        .as_array()
        .ok_or_else(|| format!("{backend} memory_layout is not an array"))?;
    if zones.is_empty() {
        return Err(format!("{backend} memory_layout is empty"));
    }
    let mut names = HashSet::new();
    for zone in zones {
        let name = zone["name"]
            .as_str()
            .ok_or_else(|| format!("{backend} zone has no name"))?;
        if !names.insert(name) {
            return Err(format!("{backend} zone name `{name}` is duplicated"));
        }
        for field in [
            "type",
            "size",
            "size_bytes",
            "read",
            "write",
            "scope",
            "role",
            "alignment",
            "runtime_allocated",
            "allocation_phase",
            "allocation_order",
            "size_exact",
            "size_source",
        ] {
            if zone.get(field).is_none() {
                return Err(format!("{backend} zone `{name}` misses `{field}`"));
            }
        }
    }
    let costs = value["compute_cost"]
        .as_array()
        .ok_or_else(|| format!("{backend} compute_cost is not an array"))?;
    if costs.len() != 1 {
        return Err(format!("{backend} compute_cost must contain one object"));
    }
    for key in ["binop", "mathop"] {
        let map = costs[0][key][0]
            .as_object()
            .ok_or_else(|| format!("{backend} {key} breakdown is invalid"))?;
        let total = map["total"]
            .as_u64()
            .ok_or_else(|| format!("{backend} {key}.total is invalid"))?;
        let sum: u64 = map
            .iter()
            .filter(|(name, _)| name.as_str() != "total")
            .map(|(_, count)| count.as_u64().unwrap_or(u64::MAX))
            .try_fold(0_u64, u64::checked_add)
            .ok_or_else(|| format!("{backend} {key} breakdown overflow"))?;
        if total != sum {
            return Err(format!("{backend} {key}.total does not match its entries"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{run, validate_document};

    fn valid_document(backend: &str) -> Value {
        let manager_abi = if backend == "cpp" {
            "dsp_memory_manager_v1"
        } else {
            "faust_memory_manager_v1"
        };
        json!({
            "memory_layout_version": 2,
            "memory_manager": {
                "mode": "mem0",
                "backend": backend,
                "manager_abi": manager_abi
            },
            "compute_cost_version": 2,
            "compute_cost_metric": "static_scalar_fir_structure",
            "compute_body_lowered": true,
            "memory_layout": [{
                "name": "dsp",
                "type": "object",
                "size": 8,
                "size_bytes": 8,
                "read": 0,
                "write": 0,
                "scope": "instance",
                "role": "main_object",
                "alignment": 8,
                "runtime_allocated": true,
                "allocation_phase": "instance_create",
                "allocation_order": 0,
                "size_exact": true,
                "size_source": "target_abi"
            }],
            "compute_cost": [{
                "binop": [{"add": 2, "mul": 1, "total": 3}],
                "mathop": [{"sin": 1, "total": 1}]
            }]
        })
    }

    #[test]
    fn checker_requires_at_least_one_document() {
        assert!(run(Vec::new()).is_err());
    }

    #[test]
    fn checker_rejects_malformed_arguments() {
        assert!(run(["cpp".to_owned()]).is_err());
    }

    #[test]
    fn checker_accepts_the_versioned_semantic_contract() {
        for backend in ["cpp", "c", "cranelift"] {
            validate_document(backend, &valid_document(backend)).expect("valid mem0 JSON");
        }
    }

    #[test]
    fn checker_rejects_a_cost_total_that_disagrees_with_its_breakdown() {
        let mut document = valid_document("c");
        document["compute_cost"][0]["binop"][0]["total"] = json!(4);
        assert!(validate_document("c", &document).is_err());
    }

    #[test]
    fn checker_layout_comparison_is_backend_local() {
        let cpp = valid_document("cpp");
        let mut c = valid_document("c");
        c["memory_layout"][0]["size_bytes"] = json!(16);
        assert_ne!(cpp["memory_layout"], c["memory_layout"]);
        validate_document("cpp", &cpp).unwrap();
        validate_document("c", &c).unwrap();
    }
}
