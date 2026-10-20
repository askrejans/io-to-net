pub mod loader;
pub mod schema;

pub use schema::*;

/// The configuration as JSON with every token replaced by `********`.
pub fn masked(cfg: &AppConfig) -> serde_json::Value {
    fn walk(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m.iter_mut() {
                    if k == "token" && v.is_string() {
                        *v = "********".into();
                    } else {
                        walk(v);
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(cfg).unwrap_or_default();
    walk(&mut v);
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn tokens_are_masked() {
        let cfg: super::AppConfig = toml::from_str(
            "[http_api]\nauth = { token = \"api-secret\" }\n[[bridges]]\nname = \"a\"\nauth = { token = \"bridge-secret\" }\n[bridges.source]\ntype = \"serial\"\nport = \"/dev/ttyUSB0\"\n[bridges.transport]\ntype = \"tcp\"\nlisten = \"127.0.0.1:9000\"\n",
        )
        .unwrap();
        let text = super::masked(&cfg).to_string();
        assert!(!text.contains("secret"), "{text}");
        assert!(text.contains("/dev/ttyUSB0"));
    }
}
