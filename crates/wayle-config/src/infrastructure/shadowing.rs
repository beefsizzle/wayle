//! Finds `runtime.toml` values that silently win over an explicit `config.toml` value.

use std::collections::BTreeMap;

use toml::Value;

/// Config paths set in both files whose values differ, sorted and deduplicated.
///
/// `runtime.toml` (written by the settings GUI and `wayle config set`) outranks `config.toml`,
/// so these are the edits in `config.toml` that currently have no effect. Keys that only exist
/// in `runtime.toml` are ordinary GUI settings and are not reported. Arrays, including arrays
/// of tables such as `[[bar.layout]]`, are one value.
///
/// `defaults` is the serialized default config. User-keyed maps (`workspace-map`,
/// `app-icon-map`, ...) are tables whose keys the schema does not know, so each reported path
/// is cut where it leaves the default config's tables. That yields the property name, which
/// `wayle config reset` accepts, instead of an invalid `app-icon-map.app:org.mozilla.*`.
pub(crate) fn shadowed_keys(config: &Value, runtime: &Value, defaults: &Value) -> Vec<String> {
    let config_leaves = leaves(config);

    let mut keys: Vec<String> = leaves(runtime)
        .into_iter()
        .filter(|(path, runtime_value)| {
            config_leaves
                .get(path)
                .is_some_and(|config_value| !values_equal(config_value, runtime_value))
        })
        .map(|(path, _)| property_path(&path, defaults))
        .collect();

    keys.sort();
    keys.dedup();
    keys
}

/// The longest prefix of `path` that the default config's tables contain.
fn property_path(path: &str, defaults: &Value) -> String {
    let mut node = defaults;
    let mut kept: Vec<&str> = Vec::new();

    for segment in path.split('.') {
        let Some(child) = node.get(segment) else {
            break;
        };

        kept.push(segment);
        node = child;

        if !node.is_table() {
            break;
        }
    }

    if kept.is_empty() {
        path.to_owned()
    } else {
        kept.join(".")
    }
}

fn leaves(value: &Value) -> BTreeMap<String, &Value> {
    let mut out = BTreeMap::new();
    collect_leaves(value, &mut String::new(), &mut out);
    out
}

fn collect_leaves<'a>(value: &'a Value, path: &mut String, out: &mut BTreeMap<String, &'a Value>) {
    match value {
        Value::Table(table) => {
            for (key, child) in table {
                let restore = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                collect_leaves(child, path, out);
                path.truncate(restore);
            }
        }
        leaf if !path.is_empty() => {
            out.insert(path.clone(), leaf);
        }
        _ => {}
    }
}

/// Floats compare with a small tolerance: `runtime.toml` stores `f32` settings widened to
/// `f64` (`0.9` is saved as `0.8999999761581421`), which must not look different. Integers
/// compare exactly, and arrays element by element so the same rule reaches floats inside them.
fn values_equal(config: &Value, runtime: &Value) -> bool {
    match (config, runtime) {
        (Value::Integer(left), Value::Integer(right)) => left == right,
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| values_equal(left, right))
        }
        (Value::Table(left), Value::Table(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| values_equal(left, right))
                })
        }
        _ => match (as_number(config), as_number(runtime)) {
            (Some(left), Some(right)) => (left - right).abs() <= 1e-5 * left.abs().max(1.0),
            _ => config == runtime,
        },
    }
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(int) => Some(*int as f64),
        Value::Float(float) => Some(*float),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_defaults() -> Value {
        Value::Table(toml::map::Map::new())
    }

    fn table(source: &str) -> Value {
        toml::from_str(source).unwrap()
    }

    #[test]
    fn reports_keys_set_in_both_files_with_different_values() {
        let config = table(
            r#"
            [bar]
            location = "top"
            [styling]
            rounding = "none"
            "#,
        );
        let runtime = table(
            r#"
            [bar]
            location = "bottom"
            [styling]
            rounding = "none"
            "#,
        );

        assert_eq!(
            shadowed_keys(&config, &runtime, &no_defaults()),
            vec!["bar.location"]
        );
    }

    #[test]
    fn ignores_runtime_keys_that_config_does_not_set() {
        let config = table("[bar]\nlocation = \"top\"");
        let runtime = table("[osd]\nmonitor = \"DP-1\"");

        assert!(shadowed_keys(&config, &runtime, &no_defaults()).is_empty());
    }

    #[test]
    fn ignores_runtime_values_equal_to_config() {
        let config = table("[bar]\nlocation = \"top\"");
        let runtime = table("[bar]\nlocation = \"top\"");

        assert!(shadowed_keys(&config, &runtime, &no_defaults()).is_empty());
    }

    #[test]
    fn widened_f32_values_are_not_a_difference() {
        let config = table("[bar]\nscale = 0.9\ngap = 1");
        let runtime = table("[bar]\nscale = 0.8999999761581421\ngap = 1.0");

        assert!(shadowed_keys(&config, &runtime, &no_defaults()).is_empty());
    }

    #[test]
    fn genuinely_different_numbers_are_reported() {
        let config = table("[bar]\nscale = 0.9");
        let runtime = table("[bar]\nscale = 1.1");

        assert_eq!(
            shadowed_keys(&config, &runtime, &no_defaults()),
            vec!["bar.scale"]
        );
    }

    #[test]
    fn arrays_compare_as_a_whole() {
        let config = table(
            r#"
            [[bar.layout]]
            monitor = "*"
            left = ["clock"]
            "#,
        );
        let runtime = table(
            r#"
            [[bar.layout]]
            monitor = "*"
            left = ["clock", "volume"]
            "#,
        );

        assert_eq!(
            shadowed_keys(&config, &runtime, &no_defaults()),
            vec!["bar.layout"]
        );
    }

    #[test]
    fn output_is_sorted_by_path() {
        let config = table("b = 1\na = 1\n[c]\nd = 1");
        let runtime = table("b = 2\na = 2\n[c]\nd = 2");

        assert_eq!(
            shadowed_keys(&config, &runtime, &no_defaults()),
            vec!["a", "b", "c.d"]
        );
    }

    #[test]
    fn integers_compare_exactly() {
        let config = table("[bar]\nopacity = 100000");
        let runtime = table("[bar]\nopacity = 100001");

        assert_eq!(
            shadowed_keys(&config, &runtime, &no_defaults()),
            vec!["bar.opacity"]
        );
    }

    #[test]
    fn widened_floats_inside_arrays_are_not_a_difference() {
        let config = table("[bar]\nweights = [0.9, 1]");
        let runtime = table("[bar]\nweights = [0.8999999761581421, 1]");

        assert!(shadowed_keys(&config, &runtime, &no_defaults()).is_empty());
    }

    #[test]
    fn map_valued_properties_report_the_property_not_the_user_key() {
        let defaults = table(
            r#"
            [modules.niri-workspaces]
            divider = " "
            [modules.niri-workspaces.app-icon-map]
            "#,
        );
        let config = table(
            r#"
            [modules.niri-workspaces.app-icon-map]
            "app:org.mozilla.*" = "ld-globe-symbolic"
            web = "a"
            [modules.niri-workspaces]
            divider = "-"
            "#,
        );
        let runtime = table(
            r#"
            [modules.niri-workspaces.app-icon-map]
            "app:org.mozilla.*" = "other"
            web = "b"
            [modules.niri-workspaces]
            divider = "+"
            "#,
        );

        assert_eq!(
            shadowed_keys(&config, &runtime, &defaults),
            vec![
                "modules.niri-workspaces.app-icon-map",
                "modules.niri-workspaces.divider",
            ]
        );
    }

    #[test]
    fn nested_property_paths_stay_whole() {
        let defaults = table("[styling.palette]\nbg = \"#000000\"");
        let config = table("[styling.palette]\nbg = \"#111111\"");
        let runtime = table("[styling.palette]\nbg = \"#222222\"");

        assert_eq!(
            shadowed_keys(&config, &runtime, &defaults),
            vec!["styling.palette.bg"]
        );
    }
}
