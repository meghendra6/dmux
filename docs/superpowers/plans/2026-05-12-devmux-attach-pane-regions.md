# Devmux Attach Pane Regions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add tested internal pane-region metadata to the server-side attach layout renderer without enabling mouse focus yet.

**Architecture:** Keep region mapping in `src/server.rs`, where layout composition already happens. Refactor the renderer so it returns `RenderedAttachLayout { lines, regions }`, then keep `render_attach_pane_snapshot` as the existing text-only wrapper for protocol compatibility.

**Tech Stack:** Rust standard library only, existing `LayoutNode`, existing split renderer helpers, existing integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Add Region Metadata To Horizontal And Vertical Rendering

**Files:**
- Modify: `src/server.rs`

- [ ] **Step 1: Write failing unit tests**

Add these tests near the existing `render_attach_layout_joins_horizontal_panes`
and `render_attach_layout_stacks_vertical_panes` tests in `src/server.rs`:

```rust
#[test]
fn render_attach_layout_maps_horizontal_pane_regions() {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        first: Box::new(LayoutNode::Pane(0)),
        second: Box::new(LayoutNode::Pane(1)),
    };
    let panes = vec![
        PaneSnapshot {
            index: 0,
            screen: "left\n".to_string(),
        },
        PaneSnapshot {
            index: 1,
            screen: "right\n".to_string(),
        },
    ];

    let rendered = render_attach_layout(&layout, &panes).unwrap();

    assert_eq!(
        rendered.regions,
        vec![
            PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 4,
            },
            PaneRegion {
                pane: 1,
                row_start: 0,
                row_end: 1,
                col_start: 7,
                col_end: 12,
            },
        ]
    );
}

#[test]
fn render_attach_layout_maps_vertical_pane_regions() {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        first: Box::new(LayoutNode::Pane(0)),
        second: Box::new(LayoutNode::Pane(1)),
    };
    let panes = vec![
        PaneSnapshot {
            index: 0,
            screen: "top\n".to_string(),
        },
        PaneSnapshot {
            index: 1,
            screen: "bottom\n".to_string(),
        },
    ];

    let rendered = render_attach_layout(&layout, &panes).unwrap();

    assert_eq!(
        rendered.regions,
        vec![
            PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 6,
            },
            PaneRegion {
                pane: 1,
                row_start: 2,
                row_end: 3,
                col_start: 0,
                col_end: 6,
            },
        ]
    );
}
```

- [ ] **Step 2: Run unit tests to verify RED**

Run:

```bash
cargo test render_attach_layout_maps_horizontal_pane_regions
cargo test render_attach_layout_maps_vertical_pane_regions
```

Expected: FAIL because `render_attach_layout`, `RenderedAttachLayout`, and `PaneRegion` do not exist.

- [ ] **Step 3: Add region structs and renderer wrapper**

In `src/server.rs`, near `PaneSnapshot`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedAttachLayout {
    lines: Vec<String>,
    regions: Vec<PaneRegion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaneRegion {
    pane: usize,
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
}
```

Refactor `render_attach_pane_snapshot`:

```rust
fn render_attach_pane_snapshot(layout: &LayoutNode, panes: &[PaneSnapshot]) -> String {
    match render_attach_layout(layout, panes) {
        Some(rendered) => render_client_lines(&rendered.lines),
        None => render_ordered_pane_sections(panes),
    }
}

fn render_attach_layout(layout: &LayoutNode, panes: &[PaneSnapshot]) -> Option<RenderedAttachLayout> {
    if !layout_matches_panes(layout, panes) {
        return None;
    }

    let screens = panes
        .iter()
        .map(|pane| (pane.index, pane.screen.as_str()))
        .collect::<HashMap<_, _>>();

    render_layout(layout, &screens)
}
```

- [ ] **Step 4: Implement recursive rendering with regions**

Replace `render_layout_lines` calls with:

```rust
fn render_layout(
    layout: &LayoutNode,
    screens: &HashMap<usize, &str>,
) -> Option<RenderedAttachLayout> {
    match layout {
        LayoutNode::Pane(index) => {
            let lines = screen_lines(screens.get(index)?);
            let width = max_line_width(&lines);
            let height = lines.len().max(1);
            Some(RenderedAttachLayout {
                lines,
                regions: vec![PaneRegion {
                    pane: *index,
                    row_start: 0,
                    row_end: height,
                    col_start: 0,
                    col_end: width,
                }],
            })
        }
        LayoutNode::Split {
            direction,
            first,
            second,
        } => {
            let first = render_layout(first, screens)?;
            let second = render_layout(second, screens)?;
            Some(match direction {
                SplitDirection::Horizontal => join_horizontal_layout(first, second),
                SplitDirection::Vertical => join_vertical_layout(first, second),
            })
        }
    }
}
```

Add these helpers:

```rust
fn join_horizontal_layout(
    left: RenderedAttachLayout,
    right: RenderedAttachLayout,
) -> RenderedAttachLayout {
    let left_width = max_line_width(&left.lines);
    let rows = left.lines.len().max(right.lines.len()).max(1);
    let lines = join_horizontal(left.lines, right.lines);

    let left_height = left.lines.len().max(1);
    let right_height = right.lines.len().max(1);
    let mut regions = expand_boundary_region_rows(left.regions, left_height, rows);
    regions.extend(offset_regions(
        expand_boundary_region_rows(right.regions, right_height, rows),
        0,
        left_width + 3,
    ));
    RenderedAttachLayout { lines, regions }
}

fn join_vertical_layout(
    top: RenderedAttachLayout,
    bottom: RenderedAttachLayout,
) -> RenderedAttachLayout {
    let width = max_line_width(&top.lines)
        .max(max_line_width(&bottom.lines))
        .max(1);
    let top_height = top.lines.len().max(1);
    let top_width = max_line_width(&top.lines);
    let bottom_width = max_line_width(&bottom.lines);
    let lines = join_vertical(top.lines, bottom.lines);

    let mut regions = expand_boundary_region_cols(top.regions, top_width, width);
    regions.extend(offset_regions(
        expand_boundary_region_cols(bottom.regions, bottom_width, width),
        top_height + 1,
        0,
    ));

    RenderedAttachLayout { lines, regions }
}

fn expand_boundary_region_rows(
    mut regions: Vec<PaneRegion>,
    current_row_end: usize,
    target_row_end: usize,
) -> Vec<PaneRegion> {
    for region in &mut regions {
        if region.row_end == current_row_end {
            region.row_end = target_row_end;
        }
    }
    regions
}

fn expand_boundary_region_cols(
    mut regions: Vec<PaneRegion>,
    current_col_end: usize,
    target_col_end: usize,
) -> Vec<PaneRegion> {
    for region in &mut regions {
        if region.col_end == current_col_end {
            region.col_end = target_col_end;
        }
    }
    regions
}

fn offset_regions(
    mut regions: Vec<PaneRegion>,
    row_offset: usize,
    col_offset: usize,
) -> Vec<PaneRegion> {
    for region in &mut regions {
        region.row_start += row_offset;
        region.row_end += row_offset;
        region.col_start += col_offset;
        region.col_end += col_offset;
    }
    regions
}
```

- [ ] **Step 5: Run unit tests to verify GREEN**

Run:

```bash
cargo test render_attach_layout_maps_horizontal_pane_regions
cargo test render_attach_layout_maps_vertical_pane_regions
cargo test render_attach_layout_joins_horizontal_panes
cargo test render_attach_layout_stacks_vertical_panes
```

Expected: PASS.

- [ ] **Step 6: Commit region renderer foundation**

Run:

```bash
git add src/server.rs
git commit -m "feat: map attach pane regions"
```

### Task 2: Cover Nested And Fallback Region Semantics

**Files:**
- Modify: `src/server.rs`
- Modify: `README.md`

- [ ] **Step 1: Write failing unit tests for nested and fallback behavior**

Add:

```rust
#[test]
fn render_attach_layout_offsets_nested_pane_regions() {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        first: Box::new(LayoutNode::Pane(0)),
        second: Box::new(LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first: Box::new(LayoutNode::Pane(1)),
            second: Box::new(LayoutNode::Pane(2)),
        }),
    };
    let panes = vec![
        PaneSnapshot {
            index: 0,
            screen: "left\n".to_string(),
        },
        PaneSnapshot {
            index: 1,
            screen: "top\n".to_string(),
        },
        PaneSnapshot {
            index: 2,
            screen: "bottom\n".to_string(),
        },
    ];

    let rendered = render_attach_layout(&layout, &panes).unwrap();

    assert_eq!(
        rendered.regions,
        vec![
            PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 3,
                col_start: 0,
                col_end: 4,
            },
            PaneRegion {
                pane: 1,
                row_start: 0,
                row_end: 1,
                col_start: 7,
                col_end: 13,
            },
            PaneRegion {
                pane: 2,
                row_start: 2,
                row_end: 3,
                col_start: 7,
                col_end: 13,
            },
        ]
    );
}

#[test]
fn render_attach_layout_returns_none_when_layout_omits_visible_pane() {
    let layout = LayoutNode::Pane(0);
    let panes = vec![
        PaneSnapshot {
            index: 0,
            screen: "base-ready\n".to_string(),
        },
        PaneSnapshot {
            index: 1,
            screen: "split-ready\n".to_string(),
        },
    ];

    assert!(render_attach_layout(&layout, &panes).is_none());
    let rendered = render_attach_pane_snapshot(&layout, &panes);
    assert!(rendered.contains("-- pane 0 --"), "{rendered:?}");
    assert!(rendered.contains("-- pane 1 --"), "{rendered:?}");
}
```

- [ ] **Step 2: Run tests to verify RED or guard current semantics**

Run:

```bash
cargo test render_attach_layout_offsets_nested_pane_regions
cargo test render_attach_layout_returns_none_when_layout_omits_visible_pane
```

Expected: PASS if Task 1 already implemented general recursion correctly; otherwise FAIL until nested offsets and fallback `None` semantics are fixed.

- [ ] **Step 3: Fix nested/fallback behavior if needed**

If nested offsets fail, adjust `join_horizontal_layout`, `join_vertical_layout`,
`expand_boundary_region_rows`, `expand_boundary_region_cols`, or `offset_regions`
so nested regions match the expected zero-based exclusive coordinate ranges.

Also add coverage for a vertical split on the left side of a horizontal split,
for example `(pane0 over pane1) | pane2`, to prove top and bottom nested regions
do not overlap when the horizontal join expands row padding.

If fallback behavior fails, ensure `render_attach_layout` returns `None` when
`layout_matches_panes(layout, panes)` is false and `render_attach_pane_snapshot`
uses `render_ordered_pane_sections(panes)` in that case.

- [ ] **Step 4: Update README**

Add implemented groundwork:

```text
- attach layout pane-region mapping foundation
```

Keep the current limit:

```text
- multi-pane attach live redraw is polling-based and routes input to the server active pane; mouse focus and composed-layout copy-mode are not implemented yet
```

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo test render_attach_layout_
cargo test --test phase1_cli attach_renders_split_pane_snapshot
cargo test --test phase1_cli attach_renders_vertical_split_layout_snapshot
cargo test --test phase1_cli attach_layout_snapshot_reindexes_after_killing_middle_pane
```

Expected: PASS. The `phase1_cli` commands may require escalation in this environment.

- [ ] **Step 6: Commit nested coverage and docs**

Run:

```bash
git add src/server.rs README.md
git commit -m "test: cover attach pane region semantics"
```

### Task 3: Verification, Reviews, PR, Merge

**Files:**
- Modify: `HANDOFF.md`
- Modify: `docs/superpowers/specs/2026-05-12-devmux-attach-pane-regions-design.md`
- Modify: `docs/superpowers/plans/2026-05-12-devmux-attach-pane-regions.md`

- [ ] **Step 1: Update HANDOFF progress**

Record branch, scope, tests run, subagent review results, PR number, merge
status, and retrospective notes. Do not stage or commit `HANDOFF.md`.

- [ ] **Step 2: Run full verification before PR**

Run:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

Expected: formatting and whitespace checks pass; reserved keyword scan prints no matches; all unit and integration tests pass. Use escalation for `cargo test` if sandboxed PTY/server integration tests fail with server readiness errors.

- [ ] **Step 3: Run subagent-driven review gates before push**

Dispatch a spec compliance subagent against `git diff origin/main..HEAD` and
the attach pane regions spec. Fix valid findings and re-review.

After spec compliance passes, dispatch a code-quality subagent against the same
diff. Fix valid Critical/Important findings and re-review if needed.

- [ ] **Step 4: Push and open PR**

Run:

```bash
git push -u origin devmux-attach-pane-regions
gh pr create --base main --head devmux-attach-pane-regions --title "Map attach pane regions" --body $'## Summary\n- Add internal pane-region metadata to the attach layout renderer.\n- Cover horizontal, vertical, nested, and fallback region semantics.\n- Keep mouse focus pending; no protocol or input dispatch changes.\n\n## Validation\n- cargo fmt --check\n- git diff --check origin/main\n- reserved keyword scan: no matches\n- cargo test\n\n## Review\n- Spec compliance and code-quality subagent reviews completed before PR.\n- Critical review will run after PR creation per workflow.'
```

The PR title and body must not contain the reserved assistant/project keyword requested by the user.

- [ ] **Step 5: Run required critical subagent review after PR creation**

Dispatch a read-only critical subagent review against `git diff origin/main..HEAD`
and PR metadata. Ask it to focus on:

```text
pane region coordinate semantics, nested split offsets, separator exclusion, fallback behavior, unchanged snapshot text output, README/spec/plan consistency, test reliability, and PR title/body reserved keyword check.
```

Evaluate each finding technically. Apply valid Critical and Important findings,
rerun full verification, push fixes, and update `HANDOFF.md`.

- [ ] **Step 6: Merge PR and record retrospective**

After review-driven fixes and verification:

```bash
PR_NUMBER=$(gh pr view --head devmux-attach-pane-regions --json number -q .number)
gh pr checks "$PR_NUMBER" --watch=false
gh pr merge "$PR_NUMBER" --squash --delete-branch --subject "Map attach pane regions"
git fetch origin --prune
```

If GitHub merges remotely but local branch update fails because `main` is owned
by another worktree, verify the PR state with:

```bash
gh pr view "$PR_NUMBER" --json state,mergedAt,mergeCommit,headRefName
```

Then delete the remote branch if needed, fetch with prune, and record the merge
commit and retrospective in `HANDOFF.md`.
