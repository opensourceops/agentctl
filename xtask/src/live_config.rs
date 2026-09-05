//! Explicit, versioned live-fixture configuration; never changes checked-in examples.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub fn settings() -> Result<(PathBuf, String)> {
    let budget = env::var_os("AGENTCTL_LIVE_BUDGET")
        .map(PathBuf::from)
        .context(
            "paid gates require AGENTCTL_LIVE_BUDGET pointing to the shared suite SQLite ledger",
        )?;
    ensure!(
        budget.is_absolute(),
        "AGENTCTL_LIVE_BUDGET must be absolute so subprocesses share one allowance"
    );
    let model = env::var("AGENTCTL_LIVE_MODEL").context(
        "paid gates require explicit AGENTCTL_LIVE_MODEL (gpt-5-mini, gpt-5.6-sol or gpt-6-astra)",
    )?;
    prices(&model)?;
    Ok((budget, model))
}

fn prices(model: &str) -> Result<(u64, u64, u64, u64)> {
    // USD per million, represented in microUSD. Public model pages checked 2026-09-06.
    // Requests stay below the long-context pricing threshold. These are estimates.
    Ok(match model {
        "gpt-5-mini" => (250_000, 2_000_000, 25_000, 250_000),
        "gpt-5.6-sol" => (4_000_000, 20_000_000, 400_000, 5_000_000),
        "gpt-6-astra" => (10_000_000, 50_000_000, 1_000_000, 12_500_000),
        _ => bail!(
            "no verified price schedule for live model `{model}`; add a reviewed schedule before paid execution"
        ),
    })
}

pub fn configure(path: &Path) -> Result<()> {
    let (_, model) = settings()?;
    let mut workflow: Value = serde_yaml_ng::from_slice(&fs::read(path)?)?;
    configure_value(&mut workflow, &model)?;
    fs::write(path, serde_json::to_vec_pretty(&workflow)?)?;
    Ok(())
}

fn configure_value(workflow: &mut Value, model: &str) -> Result<()> {
    let (input, output, cache_read, cache_write) = prices(model)?;
    let concurrency = workflow
        .pointer("/spec/runtime/maxConcurrency")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1);
    let parallel_output_cap = workflow
        .pointer("/spec/runtime/budgets/maxOutputTokens")
        .and_then(Value::as_u64)
        .map(|limit| limit / concurrency)
        .unwrap_or(2048);
    ensure!(
        parallel_output_cap > 0,
        "live output budget cannot reserve one token per concurrent task"
    );
    let providers = workflow
        .pointer("/spec/providers")
        .and_then(Value::as_object)
        .context("live workflow providers")?
        .iter()
        .filter(|(_, value)| value["kind"] == "openai")
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    ensure!(
        !providers.is_empty(),
        "live workflow has no OpenAI provider"
    );
    let agents = workflow
        .pointer_mut("/spec/agents")
        .and_then(Value::as_object_mut)
        .context("live workflow agents")?;
    for agent in agents.values_mut() {
        if providers
            .iter()
            .any(|provider| agent["provider"] == *provider)
        {
            agent["model"] = json!(model);
            // Small historical fixtures used 64 tokens, which can be consumed by reasoning.
            let cap = agent["maxOutputTokens"]
                .as_u64()
                .unwrap_or(512)
                .clamp(512, 2048)
                .min(parallel_output_cap);
            agent["maxOutputTokens"] = json!(cap);
            agent["timeoutSeconds"] =
                json!(agent["timeoutSeconds"].as_u64().unwrap_or(60).min(120));
        }
    }
    let spec = workflow["spec"]
        .as_object_mut()
        .context("live workflow spec")?;
    let runtime = spec
        .entry("runtime")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("runtime object")?;
    let budgets = runtime
        .entry("budgets")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("budgets object")?;
    for (key, cap) in [
        ("maxProviderRequests", 20_u64),
        ("maxTotalTokens", 40_000),
        ("maxWallTimeSeconds", 240),
        ("maxCostMicrousd", 2_000_000),
    ] {
        let value = budgets
            .get(key)
            .and_then(Value::as_u64)
            .unwrap_or(cap)
            .min(cap);
        budgets.insert(key.to_owned(), json!(value));
    }
    let mut models = serde_json::Map::new();
    for provider in providers {
        models.insert(
            format!("{provider}/{model}"),
            json!({
                "inputMicrousdPerMillionTokens": input,
                "outputMicrousdPerMillionTokens": output,
                "cacheReadMicrousdPerMillionTokens": cache_read,
                "cacheWriteMicrousdPerMillionTokens": cache_write,
            }),
        );
    }
    runtime.insert(
        "pricing".to_owned(),
        json!({"version":"openai-public-2026-09-06-estimate", "models": models}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_configuration_preserves_stricter_limits_and_non_openai_models() {
        let mut workflow = json!({"spec": {
            "providers":{"openai":{"kind":"openai"}, "fake":{"kind":"fake"}},
            "agents":{"a":{"provider":"openai","model":"legacy", "maxOutputTokens":64}, "b":{"provider":"fake","model":"scripted"}},
            "runtime":{"budgets":{"maxProviderRequests":1}}
        }});
        configure_value(&mut workflow, "gpt-5.6-sol").unwrap();
        assert_eq!(
            workflow["spec"]["runtime"]["budgets"]["maxProviderRequests"],
            1
        );
        assert_eq!(
            workflow["spec"]["runtime"]["budgets"]["maxTotalTokens"],
            40_000
        );
        assert_eq!(workflow["spec"]["agents"]["b"]["model"], "scripted");
        assert_eq!(workflow["spec"]["agents"]["a"]["model"], "gpt-5.6-sol");
        assert!(prices("unpriced-model").is_err());
    }

    #[test]
    fn live_configuration_preserves_aggregate_parallel_output_reservations() {
        let mut workflow: Value = serde_yaml_ng::from_str(include_str!(
            "../../examples/framework-completeness/live-composite.yaml"
        ))
        .unwrap();
        configure_value(&mut workflow, "gpt-5.6-sol").unwrap();
        assert_eq!(
            workflow["spec"]["runtime"]["budgets"]["maxOutputTokens"],
            1536
        );
        assert_eq!(workflow["spec"]["runtime"]["maxConcurrency"], 4);
        for agent in workflow["spec"]["agents"].as_object().unwrap().values() {
            assert_eq!(agent["maxOutputTokens"], 384);
        }
    }

    #[test]
    fn parallel_budget_fix_leaves_independent_live_fixture_caps_unchanged() {
        for yaml in [
            include_str!("../../examples/openai-live/workflow.yaml"),
            include_str!("../../examples/v1/openai-live.yaml"),
            include_str!("../../examples/v1/secret-reference.yaml"),
            include_str!("../../examples/docs/provider-portability/openai.yaml"),
        ] {
            let mut workflow: Value = serde_yaml_ng::from_str(yaml).unwrap();
            let before = workflow["spec"]["agents"].clone();
            configure_value(&mut workflow, "gpt-5.6-sol").unwrap();
            for (name, agent) in before.as_object().unwrap() {
                let prior_cap = agent["maxOutputTokens"]
                    .as_u64()
                    .unwrap_or(512)
                    .clamp(512, 2048);
                assert_eq!(
                    workflow["spec"]["agents"][name]["maxOutputTokens"],
                    prior_cap
                );
            }
        }
    }
}
