//! Filter and order the workspace list for display.
//!
//! Niri only keeps an empty workspace around when it is named or when it is one of
//! the auto-allocated empty ones (the trailing one on each output, plus a leading one
//! with `empty-workspace-above-first`), so every workspace niri reports is shown,
//! except:
//! - with `hide-trailing-empty`, auto-allocated ones: unnamed, empty and not active
//!   (the active one stays so the bar always has an active indicator),
//! - ones on other monitors when `monitor-specific` is on,
//! - `workspace-ignore` matches.
//!
//! No placeholder workspaces are ever fabricated.

use super::helpers;

/// Plain-data view of one workspace, used as input to [`collect_displayed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceSnapshot {
    pub id: u64,
    pub idx: u8,
    pub name: Option<String>,
    pub output: Option<String>,
    pub is_urgent: bool,
    pub is_active: bool,
    pub is_focused: bool,
    pub has_windows: bool,
}

/// Inputs that govern filtering, ordering, and padding.
#[derive(Debug, Clone)]
pub(super) struct FilterContext<'a> {
    pub monitor_specific: bool,
    pub bar_monitor: Option<&'a str>,
    pub hide_trailing_empty: bool,
    pub ignore_patterns: &'a [String],
}

pub(super) fn collect_displayed(
    snapshots: Vec<WorkspaceSnapshot>,
    ctx: &FilterContext<'_>,
) -> Vec<WorkspaceSnapshot> {
    let mut displayed: Vec<WorkspaceSnapshot> = snapshots
        .into_iter()
        .filter(|snapshot| visible_on_monitor(snapshot, ctx))
        .filter(|snapshot| {
            !helpers::is_ignored(
                snapshot.name.as_deref(),
                snapshot.idx,
                snapshot.id,
                ctx.ignore_patterns,
            )
        })
        .filter(|snapshot| !(ctx.hide_trailing_empty && is_auto_allocated(snapshot)))
        .collect();

    displayed.sort_by_key(sort_key);
    displayed
}

fn visible_on_monitor(snapshot: &WorkspaceSnapshot, ctx: &FilterContext<'_>) -> bool {
    if !ctx.monitor_specific {
        return true;
    }
    let Some(bar_monitor) = ctx.bar_monitor else {
        return true;
    };
    snapshot
        .output
        .as_deref()
        .is_some_and(|output| output == bar_monitor)
}

/// An unnamed, empty, inactive workspace is one niri allocated on its own: it removes such
/// workspaces as soon as they are left, except the ones it keeps at the edges.
fn is_auto_allocated(snapshot: &WorkspaceSnapshot) -> bool {
    snapshot.name.is_none() && !snapshot.has_windows && !snapshot.is_active
}

fn sort_key(snapshot: &WorkspaceSnapshot) -> (String, u8) {
    let output = snapshot.output.clone().unwrap_or_default();
    (output, snapshot.idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occupied(id: u64, idx: u8, output: &str) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            id,
            idx,
            name: None,
            output: Some(output.to_string()),
            is_urgent: false,
            is_active: false,
            is_focused: false,
            has_windows: true,
        }
    }

    fn named_empty(id: u64, idx: u8, output: &str, name: &str) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            id,
            idx,
            name: Some(name.to_string()),
            output: Some(output.to_string()),
            is_urgent: false,
            is_active: false,
            is_focused: false,
            has_windows: false,
        }
    }

    fn empty(id: u64, idx: u8, output: &str) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            id,
            idx,
            name: None,
            output: Some(output.to_string()),
            is_urgent: false,
            is_active: false,
            is_focused: false,
            has_windows: false,
        }
    }

    fn active_empty(id: u64, idx: u8, output: &str) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            id,
            idx,
            name: None,
            output: Some(output.to_string()),
            is_urgent: false,
            is_active: true,
            is_focused: true,
            has_windows: false,
        }
    }

    fn ctx_default<'a>() -> FilterContext<'a> {
        FilterContext {
            monitor_specific: false,
            bar_monitor: None,
            hide_trailing_empty: false,
            ignore_patterns: &[],
        }
    }

    fn ids(displayed: &[WorkspaceSnapshot]) -> Vec<u64> {
        displayed.iter().map(|snapshot| snapshot.id).collect()
    }

    #[test]
    fn hide_trailing_empty_drops_only_the_trailing_empty_workspace() {
        let snapshots = vec![
            occupied(1, 1, "DP-1"),
            named_empty(2, 2, "DP-1", "web"),
            empty(3, 3, "DP-1"),
        ];
        let ctx = FilterContext {
            hide_trailing_empty: true,
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![1, 2]);
    }

    #[test]
    fn trailing_empty_is_shown_when_hide_trailing_empty_is_off() {
        let snapshots = vec![occupied(1, 1, "DP-1"), empty(2, 2, "DP-1")];
        let displayed = collect_displayed(snapshots, &ctx_default());
        assert_eq!(ids(&displayed), vec![1, 2]);
    }

    #[test]
    fn trailing_empty_is_computed_per_output() {
        let snapshots = vec![
            occupied(1, 1, "DP-1"),
            empty(2, 2, "DP-1"),
            empty(3, 1, "DP-2"),
        ];
        let ctx = FilterContext {
            hide_trailing_empty: true,
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![1]);
    }

    #[test]
    fn named_empty_workspaces_are_shown_without_any_minimum() {
        let snapshots = vec![
            occupied(1, 1, "DP-1"),
            named_empty(2, 2, "DP-1", "chat"),
            named_empty(3, 3, "DP-1", "music"),
            empty(4, 4, "DP-1"),
        ];
        let ctx = FilterContext {
            hide_trailing_empty: true,
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![1, 2, 3]);
    }

    #[test]
    fn always_shows_active_even_when_empty() {
        let snapshots = vec![occupied(1, 1, "DP-1"), active_empty(2, 2, "DP-1")];
        let displayed = collect_displayed(snapshots, &ctx_default());
        assert_eq!(ids(&displayed), vec![1, 2]);
    }

    #[test]
    fn monitor_specific_keeps_only_this_monitors_workspaces() {
        let snapshots = vec![
            occupied(10, 1, "DP-1"),
            named_empty(20, 2, "DP-1", "2"),
            named_empty(30, 1, "DP-2", "3"),
        ];
        let ctx = FilterContext {
            monitor_specific: true,
            bar_monitor: Some("DP-1"),
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![10, 20]);
    }

    #[test]
    fn workspace_ignore_drops_by_name_glob() {
        let snapshots = vec![
            occupied(1, 1, "DP-1"),
            occupied(2, 2, "DP-1"),
            named_empty(3, 3, "DP-1", "scratch"),
        ];
        let patterns = vec![String::from("scratch")];
        let ctx = FilterContext {
            ignore_patterns: &patterns,
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![1, 2]);
    }

    #[test]
    fn active_trailing_empty_workspace_stays_visible_when_hiding_trailing_empty() {
        let snapshots = vec![occupied(1, 1, "DP-1"), active_empty(2, 2, "DP-1")];
        let ctx = FilterContext {
            hide_trailing_empty: true,
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![1, 2]);
    }

    #[test]
    fn monitor_with_only_its_active_empty_workspace_still_shows_it() {
        let snapshots = vec![occupied(1, 1, "DP-1"), active_empty(2, 1, "DP-2")];
        let ctx = FilterContext {
            monitor_specific: true,
            bar_monitor: Some("DP-2"),
            hide_trailing_empty: true,
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![2]);
    }

    #[test]
    fn leading_empty_workspace_from_empty_workspace_above_first_is_hidden() {
        let snapshots = vec![
            empty(1, 1, "DP-1"),
            occupied(2, 2, "DP-1"),
            empty(3, 3, "DP-1"),
        ];
        let ctx = FilterContext {
            hide_trailing_empty: true,
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![2]);
    }

    #[test]
    fn named_workspace_is_never_treated_as_auto_allocated() {
        let snapshots = vec![occupied(1, 1, "DP-1"), named_empty(2, 2, "DP-1", "chat")];
        let ctx = FilterContext {
            hide_trailing_empty: true,
            ..ctx_default()
        };
        let displayed = collect_displayed(snapshots, &ctx);
        assert_eq!(ids(&displayed), vec![1, 2]);
    }

    #[test]
    fn ordering_default_is_by_output_then_idx() {
        let snapshots = vec![
            occupied(3, 2, "DP-2"),
            occupied(1, 2, "DP-1"),
            occupied(2, 1, "DP-2"),
        ];
        let displayed = collect_displayed(snapshots, &ctx_default());
        assert_eq!(ids(&displayed), vec![1, 2, 3]);
    }
}
