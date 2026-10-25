# Ferrotap G2 UI Visual & Interaction Review

**Audit Date:** 2026-10-14  
**Scope:** Newly integrated G2 UI components (Custom Columns, Advanced Filter Builder, Saved Filters, Traffic Groups Panel, Filter Autocomplete, Traffic Tabs, and Layout Toggles)  
**Target Codebase:** `src/g2.rs`, `src/ui.rs`, `src/model.rs`, `src/prefs.rs`  
**Auditor:** GSD UI Reviewer  
**Overall Status:** **Action Required (Score: 2.2 / 4.0)**

---

## 1. Executive Summary

A comprehensive 6-pillar visual and interaction audit was conducted on Ferrotap's newly integrated G2 suite. While the underlying backend structures and state models in `src/g2.rs` are well-structured (featuring clean type systems for filtering, grouping, tabs, and columns), the visual presentation and frontend integration in `src/ui.rs` suffer from several architectural compromises, unfinished UI wiring, and critical UX oversights.

Key findings:
- **Traffic Groups** is rendered as a floating `egui::Window` rather than an integrated docking side panel (`egui::SidePanel`), and recalculates groupings synchronously every frame (`rebuild(&self.model.rows)` with a full `.clone()`), introducing major frame latency during active captures.
- **Filter Autocomplete & Traffic Tabs** have core model support in `g2.rs` and state fields in `FerrotapApp`, but are **completely unwired in the active UI rendering loop** (no tab bar rendered above the session table and no inline suggestion popup in filter text inputs).
- **Saved Filters** lacks destructive action safeguards (no confirmation on preset deletion `×`) and uses raw debug formatters (`Filter {:?}`) as default preset names, leading to naming collisions.
- Raw enum debug strings (`{:?}`) leak directly into user-facing UI controls (e.g. `StatusClass`, `ContentType`) instead of clean display labels.

---

## 2. 6-Pillar Evaluation & Scoring

Each pillar is scored on a standardized 1.0 to 4.0 scale:
- **1.0 - Deficient**: Broken, unusable, or absent core capabilities.
- **2.0 - Marginal**: Partially functional with visible usability barriers, layout flaws, or performance traps.
- **3.0 - Good**: Fully functional and visually coherent; minor polish items remaining.
- **4.0 - Exceptional**: Meets modern desktop standards (Fiddler/Charles-grade), pixel-perfect hierarchy, accessible and resilient.

### Pillar 1: Visual Design & Hierarchy
**Score: 2.5 / 4.0 (Marginal to Good)**
- **Strengths:**
  - Clear typography using standard `egui` headings, labels, and text styling.
  - Appropriate use of color accents: active filter rules are highlighted in `Color32::LIGHT_GREEN` for quick visual recognition.
  - Clean checkbox list layout in the Columns selector with an explicit "Reset to defaults" action.
- **Deficiencies:**
  - Group list counts in Traffic Groups are formatted as raw strings (`format!("{} ({})", g.key, count)`) without tabular alignment or secondary text dimming.
  - Raw enum debug strings leak into the UI: `selected_text(format!("{:?}", self.groups_manager.group_by.unwrap_or(GroupBy::Host)))` produces unformatted text like `StatusClass` instead of "Status Code (2xx/3xx/4xx/5xx)".
  - Active filter rule status is displayed at the bottom of the window without dedicated banner container styling.

### Pillar 2: Interaction & State Management
**Score: 2.2 / 4.0 (Marginal)**
- **Strengths:**
  - Column visibility toggles update `self.model.column_state` instantly and synchronize across table headers.
  - Advanced filter condition operators (`ConditionOp`) correctly bind to input fields.
- **Deficiencies:**
  - **Saved Filters deletion trap:** Clicking the `×` button deletes the filter preset immediately with zero confirmation prompt or undo mechanism.
  - **Default preset naming collision:** Presets saved via "Save as Preset..." use auto-generated names formatted as `format!("Filter {:?}", self.filter_builder_field)` (e.g., `Filter Host`), causing immediate name collisions when creating multiple filters on the same field.
  - **Window state leakage:** `saved_filters_window` lacks a dismissal guard (`if !self.show_saved_filters { return; }`) and does not update a tracking flag when closed via the window title bar `X`.
  - **Traffic Groups toggling:** Deselecting an active group leaves the session list filter in an ambiguous state if not explicitly cleared.

### Pillar 3: Layout & Responsiveness
**Score: 2.0 / 4.0 (Marginal)**
- **Strengths:**
  - Modal windows have sensible default widths (`default_width(280.0)`, `default_width(340.0)`).
  - Scroll areas are bound with `max_height(300.0)` to prevent infinite window expansion.
- **Deficiencies:**
  - **Floating Window vs Side Panel:** `groups_panel_window` is declared as a free-floating `egui::Window` rather than a dedicated left/right `egui::SidePanel::left("traffic_groups")`. This clutters the session grid workspace and requires manual positioning by the user.
  - **Layout toggle disconnect:** The `layout_split_horizontal` toggle exists in state and is exposed via IPC/commands, but lacks a dedicated toolbar toggle button or shortcut indicator in the main viewer panel.

### Pillar 4: Information Architecture & Density
**Score: 2.2 / 4.0 (Marginal)**
- **Strengths:**
  - Advanced Filter Builder separates field selector, operator, and value input into a coherent single-line form.
  - Preset filters list displays both name and inline quick-apply button.
- **Deficiencies:**
  - **Missing Autocomplete integration:** Although `AutocompleteIndex` collects hosts, methods, status codes, and paths in `g2.rs`, no dropdown/popup suggestions appear when typing into `filter_builder_val` or quick search boxes.
  - **Missing Traffic Tabs UI:** `TrafficTabsManager` can manage named session subsets, but `ui.rs` does not render a tab bar above the session table. Users cannot visually switch tabs or create new traffic views.

### Pillar 5: Consistency & Platform Integration
**Score: 2.4 / 4.0 (Marginal)**
- **Strengths:**
  - Native integration with `eframe`/`egui` styling, honoring system light/dark theme preferences.
  - Reuses shared session model types (`GridColumn`, `SessionRecord`) cleanly across G2 modules.
- **Deficiencies:**
  - Inconsistent window management: `columns_window`, `groups_panel_window`, and `show_filter_builder` check `if !open { self.show_* = false; }`, whereas `saved_filters_window` omits both the open check and the boolean state flag.
  - Toolbar lacks visual grouping for G2 inspection tools (Columns, Groups, Filters, Layout).

### Pillar 6: Performance & Resource Management
**Score: 1.8 / 4.0 (Deficient)**
- **Strengths:**
  - Fast in-memory evaluation of filter predicates using compiled regex and direct slice matching.
- **Deficiencies:**
  - **Critical frame-rate bottleneck in Traffic Groups:** In `groups_panel_window`, the code executes:
    ```rust
    self.groups_manager.rebuild(&self.model.rows);
    let groups = self.groups_manager.groups.clone();
    ```
    every single frame at 60 FPS. With thousands of live capture rows, iterating, grouping, allocating BTreeMaps, and cloning group vectors every frame causes noticeable UI stutter and CPU spikes.
  - Rebuilding groups should be reactive (triggered only on new incoming session events or when `group_by` changes).

---

## 3. Pillar Score Summary Table

| Pillar | Focus Area | Score (1-4) | Status |
|---|---|:---:|---|
| 1. Visual Design & Hierarchy | Typography, theme, contrast, visual cues | 2.5 | Marginal |
| 2. Interaction & State Management | Controls, feedback, deletion guards, forms | 2.2 | Marginal |
| 3. Layout & Responsiveness | Panel docking, window positioning, split layouts | 2.0 | Marginal |
| 4. Information Architecture & Density | Autocomplete suggestions, tab navigation, spacing | 2.2 | Marginal |
| 5. Consistency & Platform Integration | Window lifecycle, naming conventions, toolbar flow | 2.4 | Marginal |
| 6. Performance & Resource Management | Per-frame allocations, reactive rebuilding, memory | 1.8 | Deficient |
| **Overall Weighted Score** | **Combined UX & Technical Health** | **2.2 / 4.0** | **Action Required** |

---

## 4. Top 3 Priority Fixes

### 1. Fix Per-Frame Grouping Rebuild in Traffic Groups (Pillar 6)
- **Problem:** `self.groups_manager.rebuild(&self.model.rows)` and `.clone()` run synchronously on every egui render pass.
- **Remediation:** Introduce a `dirty: bool` flag or compare `last_row_count != self.model.rows.len()`. Only recompute groups when rows are added, cleared, or when `group_by` enum changes.

### 2. Dock Traffic Groups into `egui::SidePanel` & Humanize Labels (Pillars 1 & 3)
- **Problem:** Traffic groups floats over grid content; combo box labels display raw debug enums (`StatusClass`).
- **Remediation:** Convert `groups_panel_window` from `egui::Window` to `egui::SidePanel::left("traffic_groups_panel")`. Map enum variants to user-friendly titles ("Host", "HTTP Method", "Status Class (2xx-5xx)", "Content-Type").

### 3. Wire Traffic Tabs and Autocomplete into Active Grid View (Pillar 4)
- **Problem:** `TrafficTabsManager` and `AutocompleteIndex` exist in memory but have zero visual representation in `ui.rs`.
- **Remediation:** Add a tab bar (`ui.horizontal` with selectable tabs) directly above the session table for `tabs_manager`. Add an egui popup / combo suggestion list below the filter builder input populated by `self.autocomplete.suggest(query)`.
