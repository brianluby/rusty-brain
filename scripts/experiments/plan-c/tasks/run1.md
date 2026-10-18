Resolve the `TODO(P1)` in `crates/rb-store/src/store/core.rs` (inside `row_to_note`): links and anchors are loaded with two queries per row (N+1) in `get_many`, `list_filtered` and `memories_for_reembed`. Batch them: for a set of rows, load all their links and all their anchors with batched IN-list queries instead of two queries per row.

Requirements:
1. Results must be identical to today: each note's `links` ordered by (target_id, link_type) and `anchors` in insertion order (`memory_anchors.id`).
2. `get_many` must accept any number of ids, including more than SQLite's bound-parameter limit, while preserving request order, duplicate ids and namespace scoping.
3. `get_memory` may keep the single-row path.
4. No schema change and no new dependency.
