#![allow(missing_docs)]

//! Every combination of the styling enum options must produce CSS that GTK parses cleanly.
//!
//! The stylesheet is half static (compiled SCSS) and half generated from the config (`theme_css`),
//! and GTK only evaluates `calc(var(--x) * 1.2)` once the variable is substituted: a custom
//! property such as `--radius-none: 0` (no unit) parsed fine on its own and only failed at
//! runtime with "calc() product term has no units". So this test substitutes every `:root`
//! custom property into the full stylesheet and loads the result into a `gtk::CssProvider`,
//! failing on any `parsing-error`.
//!
//! The options come from the `EnumVariants` metadata, so a variant added to an enum is covered
//! without touching this file. The matrix is: the full product of the four rounding options and a
//! pairwise cover of all option pairs. `WAYLE_CSS_MATRIX=full` adds the full product of the other
//! options (tens of thousands of combinations, minutes) with the rounding options cycling
//! through their values.
//!
//! GTK needs a display to initialise, so the test is `#[ignore]`d. Headless run:
//!
//! ```sh
//! gtk4-broadwayd :5 &
//! GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 cargo test -p wayle-styling --test css_matrix -- --ignored
//! ```

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use wayle_config::{
    ApplyConfigLayer, CommitConfigReload, Config, EnumVariants,
    schemas::{
        bar::{BarButtonVariant, BorderLocation, IconPosition, Location, ShadowPreset},
        styling::{FontWeightClass, RoundingLevel},
    },
};
use wayle_styling::{STATIC_CSS, theme_css};

/// One config option and every value it can take.
struct Axis {
    /// Table the option lives in (`bar` or `styling`) and its key.
    table: &'static str,
    key: &'static str,
    values: Vec<&'static str>,
}

fn values<T: EnumVariants>() -> Vec<&'static str> {
    T::variants().iter().map(|variant| variant.value).collect()
}

fn axis<T: EnumVariants>(table: &'static str, key: &'static str) -> Axis {
    Axis {
        table,
        key,
        values: values::<T>(),
    }
}

/// The rounding options, which feed `calc()` expressions in the stylesheet.
fn rounding_axes() -> Vec<Axis> {
    vec![
        axis::<RoundingLevel>("styling", "rounding"),
        axis::<RoundingLevel>("bar", "rounding"),
        axis::<RoundingLevel>("bar", "button-rounding"),
        axis::<RoundingLevel>("bar", "button-group-rounding"),
    ]
}

/// Every other enum option that changes the generated CSS.
fn other_axes() -> Vec<Axis> {
    vec![
        axis::<Location>("bar", "location"),
        axis::<BorderLocation>("bar", "border-location"),
        axis::<ShadowPreset>("bar", "shadow"),
        axis::<BarButtonVariant>("bar", "button-variant"),
        axis::<FontWeightClass>("bar", "button-label-weight"),
        axis::<IconPosition>("bar", "button-icon-position"),
        axis::<BorderLocation>("bar", "button-border-location"),
        axis::<BorderLocation>("bar", "button-group-border-location"),
    ]
}

/// Builds a config with `choice[i]` applied to `axes[i]`.
fn config_for(axes: &[Axis], choice: &[usize]) -> Config {
    let mut tables: HashMap<&str, toml::Table> = HashMap::new();
    for (axis, &pick) in axes.iter().zip(choice) {
        tables.entry(axis.table).or_default().insert(
            axis.key.to_owned(),
            toml::Value::String(axis.values[pick].to_owned()),
        );
    }

    let mut root = toml::Table::new();
    for (name, table) in tables {
        root.insert(name.to_owned(), toml::Value::Table(table));
    }

    let config = Config::default();
    config.apply_config_layer(&toml::Value::Table(root), "");
    config.commit_config_reload();
    config
}

/// A misspelt key is ignored by the config layer and would silently test the defaults, so
/// check the option really took the requested value.
fn assert_applied(
    config: &Config,
    axes: &[Axis],
    choice: &[usize],
) -> Result<(), toml::ser::Error> {
    let serialized = toml::Value::try_from(config)?;
    for (axis, &pick) in axes.iter().zip(choice) {
        let actual = serialized
            .get(axis.table)
            .and_then(|table| table.get(axis.key))
            .and_then(toml::Value::as_str);
        assert_eq!(
            actual,
            Some(axis.values[pick]),
            "{}.{} was not applied (wrong key?)",
            axis.table,
            axis.key
        );
    }
    Ok(())
}

fn describe(axes: &[Axis], choice: &[usize]) -> String {
    axes.iter()
        .zip(choice)
        .map(|(axis, &pick)| format!("{}.{}={}", axis.table, axis.key, axis.values[pick]))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collects `--name: value;` declarations from every `:root { ... }` block, later ones winning
/// like the cascade would.
fn root_variables(css: &str) -> HashMap<String, String> {
    let mut variables = HashMap::new();
    let mut rest = css;
    while let Some(start) = rest.find(":root") {
        rest = &rest[start + ":root".len()..];
        let Some(open) = rest.find('{') else { break };
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        for declaration in rest[open + 1..open + close].split(';') {
            if let Some((name, value)) = declaration.split_once(':') {
                let name = name.trim();
                if name.starts_with("--") {
                    variables.insert(name.to_owned(), value.trim().to_owned());
                }
            }
        }
        rest = &rest[open + close..];
    }
    variables
}

/// Replaces every `var(--name)` / `var(--name, fallback)` whose variable is defined in `:root`,
/// repeatedly, so nested variables resolve. Unknown variables are left alone.
fn expand_variables(css: &str, variables: &HashMap<String, String>) -> String {
    let mut current = css.to_owned();
    for _ in 0..12 {
        let mut out = String::with_capacity(current.len());
        let mut rest = current.as_str();
        let mut changed = false;

        while let Some(start) = rest.find("var(") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 4..];

            let mut depth = 1;
            let mut end = None;
            for (index, ch) in after.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(index);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            };

            let inner = &after[..end];
            let (name, fallback) = match inner.split_once(',') {
                Some((name, fallback)) => (name.trim(), Some(fallback.trim())),
                None => (inner.trim(), None),
            };

            match variables.get(name).map(String::as_str).or(fallback) {
                Some(value) => {
                    out.push_str(value);
                    changed = true;
                }
                None => out.push_str(&rest[start..start + 4 + end + 1]),
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);

        current = out;
        if !changed {
            break;
        }
    }
    current
}

/// Parse errors GTK reports for `css`.
fn parse_errors(css: &str) -> Vec<String> {
    let errors = Rc::new(RefCell::new(Vec::new()));
    let provider = gtk4::CssProvider::new();
    let sink = Rc::clone(&errors);
    provider.connect_parsing_error(move |_, section, error| {
        sink.borrow_mut().push(format!("{section}: {error}"));
    });
    provider.load_from_string(css);
    errors.take()
}

fn check(config: &Config, label: &str, failures: &mut Vec<String>) {
    let theme = theme_css(
        &config.styling.palette(),
        &config.general,
        &config.bar,
        &config.styling,
    );
    let full = format!("{STATIC_CSS}\n{theme}");
    let expanded = expand_variables(&full, &root_variables(&full));

    let errors = parse_errors(&expanded);
    if !errors.is_empty() {
        failures.push(format!("{label}\n    {}", errors.join("\n    ")));
    }
}

/// A small all-pairs cover: every pair of values of every pair of axes appears in at least one
/// returned choice. Greedy and deterministic, so failures reproduce.
fn pairwise(sizes: &[usize]) -> Vec<Vec<usize>> {
    let mut uncovered: std::collections::BTreeSet<(usize, usize, usize, usize)> =
        Default::default();
    for a in 0..sizes.len() {
        for b in a + 1..sizes.len() {
            for va in 0..sizes[a] {
                for vb in 0..sizes[b] {
                    uncovered.insert((a, va, b, vb));
                }
            }
        }
    }

    let mut rows = Vec::new();
    while let Some(&(a, va, b, vb)) = uncovered.iter().next() {
        let mut row = vec![0; sizes.len()];
        let mut fixed = vec![false; sizes.len()];
        row[a] = va;
        row[b] = vb;
        fixed[a] = true;
        fixed[b] = true;

        for axis in 0..sizes.len() {
            if fixed[axis] {
                continue;
            }
            let best = (0..sizes[axis])
                .max_by_key(|&candidate| {
                    (0..sizes.len())
                        .filter(|&other| fixed[other])
                        .filter(|&other| {
                            let (lo, vlo, hi, vhi) = if other < axis {
                                (other, row[other], axis, candidate)
                            } else {
                                (axis, candidate, other, row[other])
                            };
                            uncovered.contains(&(lo, vlo, hi, vhi))
                        })
                        .count()
                })
                .unwrap_or(0);
            row[axis] = best;
            fixed[axis] = true;
        }

        for x in 0..sizes.len() {
            for y in x + 1..sizes.len() {
                uncovered.remove(&(x, row[x], y, row[y]));
            }
        }
        rows.push(row);
    }
    rows
}

fn cartesian(sizes: &[usize]) -> Vec<Vec<usize>> {
    let mut rows = vec![Vec::new()];
    for &size in sizes {
        rows = rows
            .into_iter()
            .flat_map(|row| {
                (0..size).map(move |value| {
                    let mut next = row.clone();
                    next.push(value);
                    next
                })
            })
            .collect();
    }
    rows
}

#[test]
fn pairwise_cover_includes_every_pair() {
    let sizes = [4, 5, 3, 4, 2];
    let rows = pairwise(&sizes);

    for a in 0..sizes.len() {
        for b in a + 1..sizes.len() {
            for va in 0..sizes[a] {
                for vb in 0..sizes[b] {
                    assert!(
                        rows.iter().any(|row| row[a] == va && row[b] == vb),
                        "pair ({a}={va}, {b}={vb}) missing"
                    );
                }
            }
        }
    }
    assert!(
        rows.len() < cartesian(&sizes).len() / 4,
        "cover should be much smaller"
    );
}

#[test]
fn expands_nested_variables_and_fallbacks() {
    let variables = root_variables(
        ":root { --a: 0px; --b: calc(var(--a) * 2); } .x { top: var(--b); left: var(--missing, 1px); right: var(--unknown); }",
    );
    let expanded = expand_variables(
        ".x { top: var(--b); left: var(--missing, 1px); right: var(--unknown); }",
        &variables,
    );

    assert_eq!(
        expanded,
        ".x { top: calc(0px * 2); left: 1px; right: var(--unknown); }"
    );
}

/// The matrix is only meaningful if GTK really rejects a unitless zero inside `calc()` once the
/// variable is substituted (it is the bug this test exists for), so prove it first.
fn assert_matrix_can_see_unitless_calc() {
    let css = ":root { --r: 0; } .a { padding: calc(var(--r) * 1.2); }";
    let expanded = expand_variables(css, &root_variables(css));

    assert!(
        !parse_errors(&expanded).is_empty(),
        "the matrix would not notice a unitless zero in calc()"
    );
}

#[test]
#[ignore = "needs a display (see the module docs for a headless run)"]
fn every_styling_option_combination_parses_cleanly() -> Result<(), Box<dyn std::error::Error>> {
    // Fails here without a display; see the module docs for a headless run.
    gtk4::init()?;
    assert_matrix_can_see_unitless_calc();

    let rounding = rounding_axes();
    let others = other_axes();
    let all: Vec<Axis> = rounding.into_iter().chain(others).collect();
    let rounding_count = 4;

    let mut failures = Vec::new();
    let mut combinations = 0;

    // The full product of the rounding options (they feed calc() expressions).
    let rounding_sizes: Vec<usize> = all[..rounding_count]
        .iter()
        .map(|axis| axis.values.len())
        .collect();
    for choice in cartesian(&rounding_sizes) {
        let mut full = choice.clone();
        full.resize(all.len(), 0);
        let config = config_for(&all, &full);
        assert_applied(&config, &all[..rounding_count], &choice)?;
        check(
            &config,
            &describe(&all[..rounding_count], &choice),
            &mut failures,
        );
        combinations += 1;
    }

    // Every pair of option values somewhere.
    let sizes: Vec<usize> = all.iter().map(|axis| axis.values.len()).collect();
    let mut rows = pairwise(&sizes);

    // Opt-in: everything but the rounding options, which cycle through their values.
    if std::env::var("WAYLE_CSS_MATRIX").is_ok_and(|mode| mode == "full") {
        for (index, other) in cartesian(&sizes[rounding_count..]).into_iter().enumerate() {
            let mut row: Vec<usize> = (0..rounding_count)
                .map(|axis| (index + axis) % sizes[axis])
                .collect();
            row.extend(other);
            rows.push(row);
        }
    }

    for choice in rows {
        let config = config_for(&all, &choice);
        assert_applied(&config, &all, &choice)?;
        check(&config, &describe(&all, &choice), &mut failures);
        combinations += 1;
    }

    eprintln!("checked {combinations} option combinations");
    assert!(
        failures.is_empty(),
        "{} of {combinations} combinations produced CSS that GTK rejects:\n{}",
        failures.len(),
        failures
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}
