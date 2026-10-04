//! Niri and Hyprland both define a `WorkspaceMap`; each must keep its own JSON schema.
//!
//! Regression: schemars names definitions by type name, so the two collided and the niri
//! property resolved to Hyprland's digits-only key pattern, rejecting named workspaces.

use serde_json::Value;
use wayle_config::infrastructure::schema::generate_schema;

fn resolve<'a>(root: &'a Value, mut node: &'a Value) -> Option<&'a Value> {
    for _ in 0..8 {
        let Some(reference) = node.get("$ref").and_then(Value::as_str) else {
            return Some(node);
        };
        node = root.pointer(reference.strip_prefix('#')?)?;
    }
    None
}

fn workspace_map_schema<'a>(root: &'a Value, config_def: &str) -> Option<&'a Value> {
    let property = root.pointer(&format!("/$defs/{config_def}/properties/workspace-map"))?;
    resolve(root, property)
}

#[test]
fn niri_workspace_map_accepts_named_workspaces() -> Result<(), Box<dyn std::error::Error>> {
    let root: Value = serde_json::from_str(&generate_schema().ok_or("schema generation failed")?)?;

    let niri = workspace_map_schema(&root, "NiriWorkspacesConfig").ok_or("niri map missing")?;

    assert!(
        niri.get("patternProperties").is_none(),
        "niri workspace-map must accept arbitrary names, got: {niri}"
    );
    assert!(
        niri.get("additionalProperties")
            .is_some_and(|v| v != &Value::Bool(false)),
        "niri workspace-map must allow additional keys, got: {niri}"
    );
    Ok(())
}

#[test]
fn hyprland_workspace_map_keeps_numeric_keys() -> Result<(), Box<dyn std::error::Error>> {
    let root: Value = serde_json::from_str(&generate_schema().ok_or("schema generation failed")?)?;

    let hyprland =
        workspace_map_schema(&root, "HyprlandWorkspacesConfig").ok_or("hyprland map missing")?;

    assert!(
        hyprland.get("patternProperties").is_some(),
        "hyprland workspace-map must keep its numeric key pattern, got: {hyprland}"
    );
    Ok(())
}
