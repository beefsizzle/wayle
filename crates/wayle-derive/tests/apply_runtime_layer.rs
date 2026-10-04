#![allow(missing_docs)]

//! One invalid runtime value must not stop the valid ones around it from loading.
//!
//! `runtime.toml` is rewritten from memory on every save, so any value that failed to load
//! because of an unrelated bad sibling is dropped from disk for good.

use wayle_config::{ApplyRuntimeLayer, ConfigProperty};
use wayle_derive::ApplyRuntimeLayer;

#[derive(ApplyRuntimeLayer)]
struct Inner {
    first: ConfigProperty<bool>,
    broken: ConfigProperty<u32>,
    last: ConfigProperty<bool>,
}

#[derive(ApplyRuntimeLayer)]
struct Outer {
    before: ConfigProperty<bool>,
    inner: Inner,
    after: ConfigProperty<bool>,
}

fn inner() -> Inner {
    Inner {
        first: ConfigProperty::new(false),
        broken: ConfigProperty::new(0),
        last: ConfigProperty::new(false),
    }
}

#[test]
fn bad_field_does_not_skip_its_siblings() {
    let config = inner();
    let runtime = toml::toml! {
        first = true
        broken = "not a number"
        last = true
    };

    let result = config.apply_runtime_layer(&toml::Value::Table(runtime), "");

    assert!(result.is_err(), "the invalid value must still be reported");
    assert_eq!(config.first.runtime(), Some(true));
    assert_eq!(config.last.runtime(), Some(true));
    assert_eq!(config.broken.runtime(), None);
}

#[test]
fn bad_nested_field_does_not_skip_outer_siblings() {
    let config = Outer {
        before: ConfigProperty::new(false),
        inner: inner(),
        after: ConfigProperty::new(false),
    };
    let runtime = toml::toml! {
        before = true
        after = true

        [inner]
        broken = "not a number"
        last = true
    };

    let result = config.apply_runtime_layer(&toml::Value::Table(runtime), "");

    assert!(result.is_err(), "the invalid value must still be reported");
    assert_eq!(config.before.runtime(), Some(true));
    assert_eq!(config.inner.last.runtime(), Some(true));
    assert_eq!(config.after.runtime(), Some(true));
}

#[test]
fn all_valid_values_report_success() {
    let config = inner();
    let runtime = toml::toml! {
        first = true
        broken = 7
    };

    let result = config.apply_runtime_layer(&toml::Value::Table(runtime), "");

    assert_eq!(result, Ok(()));
    assert_eq!(config.broken.runtime(), Some(7));
}
