//! Implements Dijkstra's algorithm for shortest path, to find an
//! optimal and readable diff between two ASTs.

use std::{cmp::Reverse, env, rc::Rc};

use crate::{
    diff::changes::ChangeMap,
    diff::graph::{neighbours, populate_change_map, Edge, Vertex},
    parse::syntax::{reversed_copy, Syntax},
};
use bumpalo::Bump;
use itertools::Itertools;
use radix_heap::RadixHeapMap;
use rustc_hash::FxHashMap;
use typed_arena::Arena;

type PredecessorInfo<'a, 'b> = (u64, &'b Vertex<'a>, Edge);

fn shortest_path(start: Vertex, size_hint: usize) -> Vec<(Edge, Vertex)> {
    // We want to visit nodes with the shortest distance first, but
    // RadixHeapMap is a max-heap. Ensure nodes are wrapped with
    // Reverse to flip comparisons.
    let mut heap: RadixHeapMap<Reverse<_>, &Vertex> = RadixHeapMap::new();

    let vertex_arena = Bump::new();
    heap.push(Reverse(0), vertex_arena.alloc(start));

    // TODO: this grows very big. Consider using IDA* to reduce memory
    // usage.
    let mut predecessors: FxHashMap<&Vertex, PredecessorInfo> = FxHashMap::default();
    predecessors.reserve(size_hint);

    let mut neighbour_buf = [
        None, None, None, None, None, None, None, None, None, None, None, None,
    ];
    let end = loop {
        match heap.pop() {
            Some((Reverse(distance), current)) => {
                if current.is_end() {
                    break current;
                }

                neighbours(current, &mut neighbour_buf, &vertex_arena);
                for neighbour in &mut neighbour_buf {
                    if let Some((edge, next)) = neighbour.take() {
                        let distance_to_next = distance + edge.cost();
                        let found_shorter_route = match predecessors.get(&next) {
                            Some((prev_shortest, _, _)) => distance_to_next < *prev_shortest,
                            _ => true,
                        };

                        if found_shorter_route {
                            predecessors.insert(next, (distance_to_next, current, edge));

                            heap.push(Reverse(distance_to_next), next);
                        }
                    }
                }
            }
            None => panic!("Ran out of graph nodes before reaching end"),
        }
    };

    debug!(
        "Found predecessors for {} vertices (hashmap key: {} bytes, value: {} bytes), with {} left on heap.",
        predecessors.len(),
        std::mem::size_of::<Rc<Vertex>>(),
        std::mem::size_of::<PredecessorInfo>(),
        heap.len(),
    );
    let mut current = end;

    let mut route: Vec<(Edge, Vertex)> = vec![];
    let mut cost = 0;
    while let Some((_, node, edge)) = predecessors.remove(&current) {
        route.push((edge, node.clone()));
        cost += edge.cost();

        current = node;
    }
    route.reverse();

    debug!("Found a path of {} with cost {}.", route.len(), cost);
    let print_length = if env::var("DFT_VERBOSE").is_ok() {
        50
    } else {
        5
    };
    debug!(
        "Initial {} items on path: {:#?}",
        print_length,
        route
            .iter()
            .map(|x| {
                format!(
                    "{:20} {:20} --- {:3} {:?}",
                    x.1.lhs_syntax
                        .map_or_else(|| "None".into(), Syntax::dbg_content),
                    x.1.rhs_syntax
                        .map_or_else(|| "None".into(), Syntax::dbg_content),
                    x.0.cost(),
                    x.0,
                )
            })
            .take(print_length)
            .collect_vec()
    );
    route
}

pub fn bidi_shortest_path<'a>(
    forward_start: Vertex<'a>,
    backward_start: Vertex<'a>,
    size_hint: usize,
) {
    let mut forward_heap: RadixHeapMap<Reverse<_>, &Vertex> = RadixHeapMap::new();
    let mut backward_heap: RadixHeapMap<Reverse<_>, &Vertex> = RadixHeapMap::new();

    let vertex_arena = Bump::new();

    forward_heap.push(Reverse(0), vertex_arena.alloc(forward_start));
    backward_heap.push(Reverse(0), vertex_arena.alloc(backward_start));

    let mut forward_predecessors: FxHashMap<&Vertex, PredecessorInfo> = FxHashMap::default();
    forward_predecessors.reserve(size_hint);
    let mut backward_predecessors: FxHashMap<&Vertex, PredecessorInfo> = FxHashMap::default();
    backward_predecessors.reserve(size_hint);

    let mut neighbour_buf = [
        None, None, None, None, None, None, None, None, None, None, None, None,
    ];

    let mid = loop {
        if forward_heap.len() <= backward_heap.len() {
            let (Reverse(distance), current) =
                forward_heap.pop().expect("Heap should be non-empty");

            if backward_predecessors.contains_key(&current) {
                break current;
            }

            neighbours(current, &mut neighbour_buf, &vertex_arena);
            for neighbour in &mut neighbour_buf {
                if let Some((edge, next)) = neighbour.take() {
                    let distance_to_next = distance + edge.cost();
                    let found_shorter_route = match forward_predecessors.get(&next) {
                        Some((prev_shortest, _, _)) => distance_to_next < *prev_shortest,
                        _ => true,
                    };

                    if found_shorter_route {
                        forward_predecessors.insert(next, (distance_to_next, current, edge));

                        forward_heap.push(Reverse(distance_to_next), next);
                    }
                }
            }
        } else {
            let (Reverse(distance), current) =
                backward_heap.pop().expect("Heap should be non-empty");

            if forward_predecessors.contains_key(&current) {
                break current;
            }

            neighbours(current, &mut neighbour_buf, &vertex_arena);
            for neighbour in &mut neighbour_buf {
                if let Some((edge, next)) = neighbour.take() {
                    let distance_to_next = distance + edge.cost();
                    let found_shorter_route = match backward_predecessors.get(&next) {
                        Some((prev_shortest, _, _)) => distance_to_next < *prev_shortest,
                        _ => true,
                    };

                    if found_shorter_route {
                        backward_predecessors.insert(next, (distance_to_next, current, edge));

                        backward_heap.push(Reverse(distance_to_next), next);
                    }
                }
            }
        }
    };

    dbg!(mid);
}

/// What is the total number of AST nodes?
fn node_count(root: Option<&Syntax>) -> u32 {
    let mut node = root;
    let mut count = 0;
    while let Some(current_node) = node {
        let current_count = match current_node {
            Syntax::List {
                num_descendants, ..
            } => *num_descendants,
            Syntax::Atom { .. } => 1,
        };
        count += current_count;

        node = current_node.next_sibling();
    }

    count
}

/// How many top-level AST nodes do we have?
fn tree_count(root: Option<&Syntax>) -> u32 {
    let mut node = root;
    let mut count = 0;
    while let Some(current_node) = node {
        count += 1;
        node = current_node.next_sibling();
    }

    count
}

pub fn mark_syntax<'a>(
    arena: &'a Arena<Syntax<'a>>,
    lhs_syntax: Option<&'a Syntax<'a>>,
    rhs_syntax: Option<&'a Syntax<'a>>,
    lhs_roots: &[&'a Syntax<'a>],
    rhs_roots: &[&'a Syntax<'a>],
    change_map: &mut ChangeMap<'a>,
) {
    let lhs_node_count = node_count(lhs_syntax) as usize;
    let rhs_node_count = node_count(rhs_syntax) as usize;
    info!(
        "LHS nodes: {} ({} toplevel), RHS nodes: {} ({} toplevel)",
        lhs_node_count,
        tree_count(lhs_syntax),
        rhs_node_count,
        tree_count(rhs_syntax),
    );

    // When there are a large number of changes, we end up building a
    // graph whose size is roughly quadratic. Use this as a size hint,
    // so we don't spend too much time re-hashing and expanding the
    // predecessors hashmap.
    let size_hint = lhs_node_count * rhs_node_count;

    let start = Vertex::new(lhs_syntax, rhs_syntax);
    let route = shortest_path(start, size_hint);

    let lhs_rev_roots = reversed_copy(arena, lhs_roots);
    let rhs_rev_roots = reversed_copy(arena, rhs_roots);
    let rev_start = Vertex::new(lhs_rev_roots.get(0).copied(), rhs_rev_roots.get(0).copied());
    let forward_start = Vertex::new(lhs_syntax, rhs_syntax);

    bidi_shortest_path(forward_start, rev_start, size_hint);

    populate_change_map(&route, change_map);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        diff::changes::ChangeKind,
        diff::graph::Edge::*,
        positions::SingleLineSpan,
        syntax::{init_all_info, AtomKind},
    };

    use itertools::Itertools;
    use typed_arena::Arena;

    fn pos_helper(line: usize) -> Vec<SingleLineSpan> {
        vec![SingleLineSpan {
            line: line.into(),
            start_col: 0,
            end_col: 1,
        }]
    }

    fn col_helper(line: usize, col: usize) -> Vec<SingleLineSpan> {
        vec![SingleLineSpan {
            line: line.into(),
            start_col: col,
            end_col: col + 1,
        }]
    }

    #[test]
    fn identical_atoms() {
        let arena = Arena::new();

        let lhs = Syntax::new_atom(&arena, pos_helper(0), "foo", AtomKind::Normal);
        // Same content as LHS.
        let rhs = Syntax::new_atom(&arena, pos_helper(0), "foo", AtomKind::Normal);
        init_all_info(&[lhs], &[rhs]);

        let start = Vertex::new(Some(lhs), Some(rhs));
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![UnchangedNode {
                depth_difference: 0
            }]
        );
    }

    #[test]
    fn extra_atom_lhs() {
        let arena = Arena::new();

        let lhs = vec![Syntax::new_list(
            &arena,
            "[",
            pos_helper(0),
            vec![Syntax::new_atom(
                &arena,
                pos_helper(1),
                "foo",
                AtomKind::Normal,
            )],
            "]",
            pos_helper(2),
        )];

        let rhs = vec![Syntax::new_list(
            &arena,
            "[",
            pos_helper(0),
            vec![],
            "]",
            pos_helper(2),
        )];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![
                EnterUnchangedDelimiter {
                    depth_difference: 0
                },
                NovelAtomLHS { contiguous: false },
                ExitDelimiterBoth,
            ]
        );
    }

    #[test]
    fn repeated_atoms() {
        let arena = Arena::new();

        let lhs = vec![Syntax::new_list(
            &arena,
            "[",
            pos_helper(0),
            vec![],
            "]",
            pos_helper(2),
        )];

        let rhs = vec![Syntax::new_list(
            &arena,
            "[",
            pos_helper(0),
            vec![
                Syntax::new_atom(&arena, pos_helper(1), "foo", AtomKind::Normal),
                Syntax::new_atom(&arena, pos_helper(2), "foo", AtomKind::Normal),
            ],
            "]",
            pos_helper(3),
        )];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![
                EnterUnchangedDelimiter {
                    depth_difference: 0
                },
                NovelAtomRHS { contiguous: false },
                NovelAtomRHS { contiguous: false },
                ExitDelimiterBoth,
            ]
        );
    }

    #[test]
    fn atom_after_empty_list() {
        let arena = Arena::new();

        let lhs = vec![Syntax::new_list(
            &arena,
            "[",
            pos_helper(0),
            vec![
                Syntax::new_list(&arena, "(", pos_helper(1), vec![], ")", pos_helper(2)),
                Syntax::new_atom(&arena, pos_helper(3), "foo", AtomKind::Normal),
            ],
            "]",
            pos_helper(4),
        )];

        let rhs = vec![Syntax::new_list(
            &arena,
            "{",
            pos_helper(0),
            vec![
                Syntax::new_list(&arena, "(", pos_helper(1), vec![], ")", pos_helper(2)),
                Syntax::new_atom(&arena, pos_helper(3), "foo", AtomKind::Normal),
            ],
            "}",
            pos_helper(4),
        )];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![
                EnterNovelDelimiterRHS { contiguous: false },
                EnterNovelDelimiterLHS { contiguous: false },
                UnchangedNode {
                    depth_difference: 0
                },
                UnchangedNode {
                    depth_difference: 0
                },
                ExitDelimiterRHS,
                ExitDelimiterLHS,
            ],
        );
    }

    #[test]
    fn prefer_atoms_same_line() {
        let arena = Arena::new();

        let lhs = vec![
            Syntax::new_atom(&arena, col_helper(1, 0), "foo", AtomKind::Normal),
            Syntax::new_atom(&arena, col_helper(2, 0), "bar", AtomKind::Normal),
            Syntax::new_atom(&arena, col_helper(2, 1), "foo", AtomKind::Normal),
        ];

        let rhs = vec![Syntax::new_atom(
            &arena,
            col_helper(1, 0),
            "foo",
            AtomKind::Normal,
        )];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![
                UnchangedNode {
                    depth_difference: 0
                },
                NovelAtomLHS { contiguous: false },
                NovelAtomLHS { contiguous: true },
            ]
        );
    }

    #[test]
    fn prefer_children_same_line() {
        let arena = Arena::new();

        let lhs = vec![Syntax::new_list(
            &arena,
            "[",
            col_helper(1, 0),
            vec![Syntax::new_atom(
                &arena,
                col_helper(1, 2),
                "1",
                AtomKind::Normal,
            )],
            "]",
            pos_helper(2),
        )];

        let rhs = vec![];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![
                EnterNovelDelimiterLHS { contiguous: false },
                NovelAtomLHS { contiguous: true },
                ExitDelimiterLHS,
            ]
        );
    }

    #[test]
    fn atom_after_novel_list_contiguous() {
        let arena = Arena::new();

        let lhs = vec![
            Syntax::new_list(
                &arena,
                "[",
                col_helper(1, 0),
                vec![Syntax::new_atom(
                    &arena,
                    col_helper(1, 2),
                    "1",
                    AtomKind::Normal,
                )],
                "]",
                col_helper(2, 1),
            ),
            Syntax::new_atom(&arena, col_helper(2, 2), ";", AtomKind::Normal),
        ];

        let rhs = vec![];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![
                EnterNovelDelimiterLHS { contiguous: false },
                NovelAtomLHS { contiguous: true },
                ExitDelimiterLHS,
                NovelAtomLHS { contiguous: true },
            ]
        );
    }

    #[test]
    fn replace_similar_comment() {
        let arena = Arena::new();

        let lhs = vec![Syntax::new_atom(
            &arena,
            pos_helper(1),
            "the quick brown fox",
            AtomKind::Comment,
        )];

        let rhs = vec![Syntax::new_atom(
            &arena,
            pos_helper(1),
            "the quick brown cat",
            AtomKind::Comment,
        )];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![ReplacedComment {
                levenshtein_pct: 84
            }]
        );
    }

    #[test]
    fn replace_very_different_comment() {
        let arena = Arena::new();

        let lhs = vec![Syntax::new_atom(
            &arena,
            pos_helper(1),
            "the quick brown fox",
            AtomKind::Comment,
        )];

        let rhs = vec![Syntax::new_atom(
            &arena,
            pos_helper(1),
            "foo bar",
            AtomKind::Comment,
        )];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![ReplacedComment {
                levenshtein_pct: 11
            }]
        );
    }

    #[test]
    fn replace_comment_prefer_most_similar() {
        let arena = Arena::new();

        let lhs = vec![
            Syntax::new_atom(
                &arena,
                pos_helper(1),
                "the quick brown fox",
                AtomKind::Comment,
            ),
            Syntax::new_atom(
                &arena,
                pos_helper(2),
                "the quick brown thing",
                AtomKind::Comment,
            ),
        ];

        let rhs = vec![Syntax::new_atom(
            &arena,
            pos_helper(1),
            "the quick brown fox.",
            AtomKind::Comment,
        )];
        init_all_info(&lhs, &rhs);

        let start = Vertex::new(lhs.get(0).copied(), rhs.get(0).copied());
        let route = shortest_path(start, 0);

        let actions = route.iter().map(|(action, _)| *action).collect_vec();
        assert_eq!(
            actions,
            vec![
                ReplacedComment {
                    levenshtein_pct: 95
                },
                NovelAtomLHS { contiguous: false }
            ]
        );
    }

    #[test]
    fn mark_syntax_equal_atoms() {
        let arena = Arena::new();
        let lhs = Syntax::new_atom(&arena, pos_helper(1), "foo", AtomKind::Normal);
        let rhs = Syntax::new_atom(&arena, pos_helper(1), "foo", AtomKind::Normal);
        init_all_info(&[lhs], &[rhs]);

        let mut change_map = ChangeMap::default();
        mark_syntax(Some(lhs), Some(rhs), &mut change_map);

        assert_eq!(change_map.get(lhs), Some(ChangeKind::Unchanged(rhs)));
        assert_eq!(change_map.get(rhs), Some(ChangeKind::Unchanged(lhs)));
    }

    #[test]
    fn mark_syntax_different_atoms() {
        let arena = Arena::new();
        let lhs = Syntax::new_atom(&arena, pos_helper(1), "foo", AtomKind::Normal);
        let rhs = Syntax::new_atom(&arena, pos_helper(1), "bar", AtomKind::Normal);
        init_all_info(&[lhs], &[rhs]);

        let mut change_map = ChangeMap::default();
        mark_syntax(Some(lhs), Some(rhs), &mut change_map);
        assert_eq!(change_map.get(lhs), Some(ChangeKind::Novel));
        assert_eq!(change_map.get(rhs), Some(ChangeKind::Novel));
    }
}
