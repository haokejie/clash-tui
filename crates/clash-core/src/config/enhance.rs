use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail};
use boa_engine::{Context, JsString, JsValue, Source, native_function::NativeFunction};
use serde_yaml_ng::{Mapping, Value};

const MAX_OUTPUTS: usize = 1000;
const MAX_OUTPUT_SIZE: usize = 1024 * 1024;
const MAX_JSON_SIZE: usize = 10 * 1024 * 1024;
const MAX_LOOP_ITERATIONS: u64 = 10_000_000;
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(5);

pub fn use_merge(merge: &Mapping, config: Mapping) -> Mapping {
    let mut config = Value::from(config);
    let merge = use_lowercase(merge);
    deep_merge(&mut config, Value::from(merge));
    config.as_mapping().cloned().unwrap_or_default()
}

pub async fn use_script(script: String, config: Mapping, name: String) -> Result<(Mapping, Vec<(String, String)>)> {
    let handle = tokio::task::spawn_blocking(move || use_script_sync(script, &config, &name));
    match tokio::time::timeout(SCRIPT_TIMEOUT, handle).await {
        Ok(Ok(result)) => result,
        Ok(Err(join_err)) => Err(anyhow!("script task panicked: {join_err}")),
        Err(_) => Err(anyhow!("script execution timed out after {SCRIPT_TIMEOUT:?}")),
    }
}

fn use_script_sync(script: String, config: &Mapping, name: &str) -> Result<(Mapping, Vec<(String, String)>)> {
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(MAX_LOOP_ITERATIONS);

    let outputs = Arc::new(Mutex::new(Vec::new()));
    let total_size = Arc::new(AtomicUsize::new(0));

    let outputs_clone = Arc::clone(&outputs);
    let total_size_clone = Arc::clone(&total_size);
    context
        .register_global_builtin_callable("__clash_tui_log__".into(), 2, unsafe {
            NativeFunction::from_closure(move |_: &JsValue, args: &[JsValue], context: &mut Context| {
                let level = js_arg_to_string(args.first(), "Missing level argument", context)?;
                let data = js_arg_to_string(args.get(1), "Missing data argument", context)?;

                let mut outputs = outputs_clone
                    .lock()
                    .map_err(|_| js_error("script output lock poisoned"))?;
                if outputs.len() >= MAX_OUTPUTS {
                    return Err(js_error("Maximum number of log outputs exceeded"));
                }

                let added_size = level.len() + data.len();
                let new_size = total_size_clone.fetch_add(added_size, Ordering::Relaxed) + added_size;
                if new_size > MAX_OUTPUT_SIZE {
                    total_size_clone.fetch_sub(added_size, Ordering::Relaxed);
                    return Err(js_error("Maximum output size exceeded"));
                }
                outputs.push((level, data));
                drop(outputs);

                Ok(JsValue::undefined())
            })
        })
        .map_err(|err| anyhow!("failed to register script console: {err}"))?;

    context
        .eval(Source::from_bytes(
            r#"var console = Object.freeze({
        log(data){__clash_tui_log__("log",JSON.stringify(data, null, 2))},
        info(data){__clash_tui_log__("info",JSON.stringify(data, null, 2))},
        error(data){__clash_tui_log__("error",JSON.stringify(data, null, 2))},
        debug(data){__clash_tui_log__("debug",JSON.stringify(data, null, 2))},
        warn(data){__clash_tui_log__("warn",JSON.stringify(data, null, 2))},
        table(data){__clash_tui_log__("table",JSON.stringify(data, null, 2))},
      });"#,
        ))
        .map_err(|err| anyhow!("failed to initialize script console: {err}"))?;

    let config = use_lowercase(config);
    let config_str = serde_json::to_string(&config).context("failed to serialize config for script")?;
    if config_str.len() > MAX_JSON_SIZE {
        bail!("Configuration size exceeds maximum allowed size");
    }

    let safe_name = escape_js_string_for_single_quote(name);
    if safe_name.len() > 1024 {
        bail!("Name parameter too long");
    }

    let code = format!(
        r"try{{
        {script};
        JSON.stringify(main({config_str},'{safe_name}')||'')
      }} catch(err) {{
        `__error_flag__ ${{err.toString()}}`
      }}"
    );

    let result = context
        .eval(Source::from_bytes(code.as_str()))
        .map_err(|err| anyhow!("script syntax error: {err}"))?;
    if !result.is_string() {
        bail!("main function should return object");
    }

    let result = result
        .to_string(&mut context)
        .map_err(|err| anyhow!("failed to convert script result to string: {err}"))?
        .to_std_string()
        .map_err(|_| anyhow!("failed to convert script result to UTF-8 string"))?;

    if let Some(message) = result.strip_prefix("__error_flag__ ") {
        bail!("script execution error: {message}");
    }
    if result.len() > MAX_JSON_SIZE {
        bail!("Script result exceeds maximum allowed size");
    }

    let config = parse_json_safely(&result)?;
    let outputs = outputs
        .lock()
        .map_err(|_| anyhow!("script output lock poisoned"))?
        .clone();

    Ok((use_lowercase(&config), outputs))
}

fn js_arg_to_string(
    value: Option<&JsValue>,
    missing_message: &str,
    context: &mut Context,
) -> std::result::Result<String, boa_engine::JsError> {
    value
        .ok_or_else(|| js_error(missing_message))?
        .to_string(context)?
        .to_std_string()
        .map_err(|_| js_error("Failed to convert value to string"))
}

fn js_error(message: &str) -> boa_engine::JsError {
    boa_engine::JsError::from_opaque(JsString::from(message).into())
}

fn parse_json_safely(json_str: &str) -> Result<Mapping> {
    if json_str.len() > MAX_JSON_SIZE {
        bail!("JSON string too large");
    }

    let json_str = strip_outer_quotes(json_str);
    serde_json::from_str::<Mapping>(json_str).context("main function should return object")
}

fn strip_outer_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() < 2 {
        return s;
    }

    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn escape_js_string_for_single_quote(s: &str) -> String {
    s.chars()
        .take(10_240)
        .flat_map(|ch| match ch {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\'' => "\\'".chars().collect(),
            '\n' => "\\n".chars().collect(),
            '\r' => "\\r".chars().collect(),
            _ => vec![ch],
        })
        .collect()
}

fn deep_merge(a: &mut Value, b: Value) {
    match (a, b) {
        (Value::Mapping(a_map), Value::Mapping(b_map)) => {
            for (key, value) in b_map {
                if let Some(existing) = a_map.get_mut(&key) {
                    deep_merge(existing, value);
                } else {
                    a_map.insert(key, value);
                }
            }
        }
        (a, b) => *a = b,
    }
}

pub fn use_lowercase(config: &Mapping) -> Mapping {
    config
        .iter()
        .map(|(key, value)| {
            let key = key
                .as_str()
                .map(|key| Value::String(key.to_ascii_lowercase()))
                .unwrap_or_else(|| key.clone());
            (key, value.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_yaml_ng::Mapping;

    use super::use_script_sync;

    #[test]
    fn use_script_adds_proxy_group() -> Result<()> {
        let script = r#"
function main(config, profileName) {
  config.proxies = Array.isArray(config.proxies) ? config.proxies : [];
  config["proxy-groups"] = Array.isArray(config["proxy-groups"]) ? config["proxy-groups"] : [];
  config.rules = Array.isArray(config.rules) ? config.rules : [];
  config.proxies.push({ name: `${profileName}-Proxy`, type: "direct" });
  config["proxy-groups"].push({ name: "LAN-Access", type: "select", proxies: [`${profileName}-Proxy`, "DIRECT"] });
  config.rules.unshift("IP-CIDR,192.168.4.0/24,LAN-Access,no-resolve");
  return config;
}
"#;
        let config: Mapping = serde_yaml_ng::from_str("proxies: []\nproxy-groups: []\nrules: []\n")?;
        let (config, _) = use_script_sync(script.into(), &config, "Demo")?;

        assert!(serde_yaml_ng::to_string(&config)?.contains("LAN-Access"));
        Ok(())
    }
}
