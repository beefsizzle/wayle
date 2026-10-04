use toml::{Value, map::Map};

/// Config paths whose arrays are concatenated across files instead of replaced.
///
/// `[[modules.custom]]` is a list of independent entries, so each imported file contributes
/// its own. Every other array follows the usual rule: the later file wins.
const CONCATENATED_ARRAYS: &[&[&str]] = &[&["modules", "custom"]];

pub(super) fn merge_toml_configs(imports: Vec<Value>, main: Value) -> Value {
    let mut accumulated = Value::Table(Map::new());

    for import in imports {
        accumulated = merge_two_toml_configs(accumulated, import);
    }

    merge_two_toml_configs(accumulated, main)
}

/// Appends `overlay` entries to `base`. An overlay entry whose `id` matches a base entry
/// replaces it in place, so redefining a module in a later file overrides the earlier
/// definition instead of being shadowed by it (consumers take the first entry for an id).
fn concat_entries(mut base: Vec<Value>, overlay: Vec<Value>) -> Vec<Value> {
    for entry in overlay {
        let existing = entry_id(&entry).and_then(|id| {
            base.iter()
                .position(|candidate| entry_id(candidate) == Some(id))
        });

        match existing {
            Some(index) => base[index] = entry,
            None => base.push(entry),
        }
    }

    base
}

fn entry_id(entry: &Value) -> Option<&str> {
    entry.get("id")?.as_str()
}

fn is_concatenated(path: &[String]) -> bool {
    CONCATENATED_ARRAYS
        .iter()
        .any(|known| known.iter().copied().eq(path.iter().map(String::as_str)))
}

fn merge_two_toml_configs(base: Value, overlay: Value) -> Value {
    merge_at(base, overlay, &mut Vec::new())
}

fn merge_at(base: Value, overlay: Value, path: &mut Vec<String>) -> Value {
    match (base, overlay) {
        (Value::Table(base_table), Value::Table(overlay_table)) => {
            let mut merged_table = overlay_table;

            for (key, base_value) in base_table {
                match merged_table.remove(&key) {
                    None => {
                        merged_table.insert(key, base_value);
                    }
                    Some(overlay_value) => {
                        path.push(key.clone());
                        let merged_value = merge_at(base_value, overlay_value, path);
                        path.pop();
                        merged_table.insert(key, merged_value);
                    }
                }
            }

            Value::Table(merged_table)
        }
        (Value::Array(base_array), Value::Array(overlay_array)) if is_concatenated(path) => {
            Value::Array(concat_entries(base_array, overlay_array))
        }
        (_, overlay) => overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod merge_two_toml_configs {
        use super::*;

        #[test]
        fn overlay_wins_for_same_key() {
            let base = toml::toml! { enabled = false };
            let overlay = toml::toml! { enabled = true };

            let result = merge_two_toml_configs(Value::Table(base), Value::Table(overlay));
            let table = result.as_table().unwrap();

            assert_eq!(table.get("enabled"), Some(&Value::Boolean(true)));
        }

        #[test]
        fn preserves_base_keys_not_in_overlay() {
            let base = toml::toml! {
                enabled = true
                count = 42
            };
            let overlay = toml::toml! {
                enabled = false
            };

            let result = merge_two_toml_configs(Value::Table(base), Value::Table(overlay));
            let table = result.as_table().unwrap();

            assert_eq!(table.get("enabled"), Some(&Value::Boolean(false)));
            assert_eq!(table.get("count"), Some(&Value::Integer(42)));
        }

        #[test]
        fn merges_nested_tables() {
            let base = toml::toml! {
                [clock]
                enabled = true
                format = "%H:%M"
            };
            let overlay = toml::toml! {
                [clock]
                format = "%I:%M %p"
            };

            let result = merge_two_toml_configs(Value::Table(base), Value::Table(overlay));
            let clock = result.get("clock").unwrap().as_table().unwrap();

            assert_eq!(clock.get("enabled"), Some(&Value::Boolean(true)));
            assert_eq!(clock.get("format"), Some(&Value::String("%I:%M %p".into())));
        }

        #[test]
        fn overlay_replaces_base_table_with_primitive() {
            let base = toml::toml! {
                [clock]
                enabled = true
            };
            let overlay = toml::toml! {
                clock = "disabled"
            };

            let result = merge_two_toml_configs(Value::Table(base), Value::Table(overlay));
            let table = result.as_table().unwrap();

            assert_eq!(table.get("clock"), Some(&Value::String("disabled".into())));
        }

        #[test]
        fn handles_empty_overlay() {
            let base = toml::toml! {
                enabled = true
                count = 42
            };
            let overlay = Map::new();

            let result = merge_two_toml_configs(Value::Table(base), Value::Table(overlay));
            let table = result.as_table().unwrap();

            assert_eq!(table.get("enabled"), Some(&Value::Boolean(true)));
            assert_eq!(table.get("count"), Some(&Value::Integer(42)));
        }

        #[test]
        fn handles_empty_base() {
            let base = Map::new();
            let overlay = toml::toml! {
                enabled = true
            };

            let result = merge_two_toml_configs(Value::Table(base), Value::Table(overlay));
            let table = result.as_table().unwrap();

            assert_eq!(table.get("enabled"), Some(&Value::Boolean(true)));
        }
    }

    mod merge_toml_configs {
        use super::*;

        fn custom_ids(value: &Value) -> Vec<&str> {
            value["modules"]["custom"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["id"].as_str().unwrap())
                .collect()
        }

        #[test]
        fn custom_modules_from_separate_imports_are_all_kept() {
            let import_a = toml::toml! {
                [[modules.custom]]
                id = "module-a"
            };
            let import_b = toml::toml! {
                [[modules.custom]]
                id = "module-b"
            };
            let main = toml::toml! {
                [[modules.custom]]
                id = "main"
            };

            let result = merge_toml_configs(
                vec![Value::Table(import_a), Value::Table(import_b)],
                Value::Table(main),
            );

            assert_eq!(custom_ids(&result), vec!["module-a", "module-b", "main"]);
        }

        fn custom_entries(value: &Value) -> Vec<(&str, Option<&str>)> {
            value["modules"]["custom"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    (
                        entry["id"].as_str().unwrap(),
                        entry.get("command").and_then(Value::as_str),
                    )
                })
                .collect()
        }

        #[test]
        fn later_file_overrides_an_earlier_custom_module_with_the_same_id() {
            let import_a = toml::toml! {
                [[modules.custom]]
                id = "shared"
                command = "from-import"
                [[modules.custom]]
                id = "only-import"
            };
            let main = toml::toml! {
                [[modules.custom]]
                id = "shared"
                command = "from-main"
                [[modules.custom]]
                id = "only-main"
            };

            let result = merge_toml_configs(vec![Value::Table(import_a)], Value::Table(main));

            assert_eq!(
                custom_entries(&result),
                vec![
                    ("shared", Some("from-main")),
                    ("only-import", None),
                    ("only-main", None),
                ]
            );
        }

        #[test]
        fn entries_without_an_id_are_never_deduplicated() {
            let import_a = toml::toml! {
                [[modules.custom]]
                command = "a"
            };
            let main = toml::toml! {
                [[modules.custom]]
                command = "b"
            };

            let result = merge_toml_configs(vec![Value::Table(import_a)], Value::Table(main));

            assert_eq!(result["modules"]["custom"].as_array().unwrap().len(), 2);
        }

        #[test]
        fn custom_modules_survive_when_only_one_file_defines_them() {
            let import_a = toml::toml! {
                [[modules.custom]]
                id = "module-a"
            };
            let main = toml::toml! {
                [modules.clock]
                format = "%H:%M"
            };

            let result = merge_toml_configs(vec![Value::Table(import_a)], Value::Table(main));

            assert_eq!(custom_ids(&result), vec!["module-a"]);
            assert_eq!(result["modules"]["clock"]["format"].as_str(), Some("%H:%M"));
        }

        #[test]
        fn other_arrays_are_still_replaced_by_the_later_file() {
            let import_a = toml::toml! {
                [[bar.layout]]
                monitor = "A"
            };
            let main = toml::toml! {
                [[bar.layout]]
                monitor = "B"
            };

            let result = merge_toml_configs(vec![Value::Table(import_a)], Value::Table(main));
            let layouts = result["bar"]["layout"].as_array().unwrap();

            assert_eq!(layouts.len(), 1);
            assert_eq!(layouts[0]["monitor"].as_str(), Some("B"));
        }

        #[test]
        fn arrays_named_custom_elsewhere_are_not_concatenated() {
            let import_a = toml::toml! {
                [other]
                custom = [1]
            };
            let main = toml::toml! {
                [other]
                custom = [2]
            };

            let result = merge_toml_configs(vec![Value::Table(import_a)], Value::Table(main));

            assert_eq!(result["other"]["custom"].as_array().unwrap().len(), 1);
        }

        #[test]
        fn main_config_takes_precedence() {
            let import1 = toml::toml! { enabled = false };
            let main = toml::toml! { enabled = true };

            let result = merge_toml_configs(vec![Value::Table(import1)], Value::Table(main));
            let table = result.as_table().unwrap();

            assert_eq!(table.get("enabled"), Some(&Value::Boolean(true)));
        }

        #[test]
        fn later_imports_override_earlier() {
            let import1 = toml::toml! { count = 1 };
            let import2 = toml::toml! { count = 2 };
            let main = Map::new();

            let result = merge_toml_configs(
                vec![Value::Table(import1), Value::Table(import2)],
                Value::Table(main),
            );
            let table = result.as_table().unwrap();

            assert_eq!(table.get("count"), Some(&Value::Integer(2)));
        }

        #[test]
        fn accumulates_all_keys() {
            let import1 = toml::toml! { key1 = "a" };
            let import2 = toml::toml! { key2 = "b" };
            let main = toml::toml! { key3 = "c" };

            let result = merge_toml_configs(
                vec![Value::Table(import1), Value::Table(import2)],
                Value::Table(main),
            );
            let table = result.as_table().unwrap();

            assert_eq!(table.get("key1"), Some(&Value::String("a".into())));
            assert_eq!(table.get("key2"), Some(&Value::String("b".into())));
            assert_eq!(table.get("key3"), Some(&Value::String("c".into())));
        }

        #[test]
        fn handles_no_imports() {
            let main = toml::toml! { enabled = true };

            let result = merge_toml_configs(vec![], Value::Table(main));
            let table = result.as_table().unwrap();

            assert_eq!(table.get("enabled"), Some(&Value::Boolean(true)));
        }
    }
}
