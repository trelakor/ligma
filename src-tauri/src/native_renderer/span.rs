//! Where a slow pass of the render loop spent its time, while tracing:
//! steps time themselves as named spans, nested as they're called, and a
//! pass that's slow is traced as a tree of them, which needs no symbols, so
//! it works in release builds too

use std::cell::RefCell;
use std::fmt::Write;
use std::time::{Duration, Instant};

struct Node {
    name: &'static str,
    took: Duration,
    count: u32,
    children: Vec<usize>,
}

struct Pass {
    began: Instant,
    nodes: Vec<Node>,
    open: Vec<usize>,
    generation: u64,
}

thread_local! {
    static PASS: RefCell<Option<Pass>> = const { RefCell::new(None) };
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Times its step till dropped, when a pass is being timed on this thread
pub struct Span(Option<(usize, u64, Instant)>);

pub fn span(name: &'static str) -> Span {
    PASS.with(|pass| {
        let mut pass = pass.borrow_mut();
        let Some(pass) = pass.as_mut() else { return Span(None) };
        let parent = pass.open.last().copied().unwrap_or(0);
        // Called again under the same parent, it adds to the same node
        let found = pass.nodes[parent].children.iter().copied().find(|&child| pass.nodes[child].name == name);
        let index = found.unwrap_or_else(|| {
            pass.nodes.push(Node { name, took: Duration::ZERO, count: 0, children: Vec::new() });
            let index = pass.nodes.len() - 1;
            pass.nodes[parent].children.push(index);
            index
        });
        pass.open.push(index);
        Span(Some((index, pass.generation, Instant::now())))
    })
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some((index, generation, at)) = self.0 else { return };
        let took = at.elapsed();
        PASS.with(|pass| {
            let mut pass = pass.borrow_mut();
            let Some(pass) = pass.as_mut().filter(|pass| pass.generation == generation) else { return };
            let node = &mut pass.nodes[index];
            node.took += took;
            node.count += 1;
            if let Some(at) = pass.open.iter().rposition(|&open| open == index) {
                pass.open.truncate(at);
            }
        });
    }
}

/// Starts timing a pass on this thread
pub fn begin() {
    let generation = GENERATION.with(|counter| {
        counter.set(counter.get() + 1);
        counter.get()
    });
    PASS.with(|pass| {
        *pass.borrow_mut() = Some(Pass {
            began: Instant::now(),
            nodes: vec![Node { name: "pass", took: Duration::ZERO, count: 1, children: Vec::new() }],
            open: Vec::new(),
            generation,
        });
    });
}

/// Ends the pass, and how long it was busy, all but `idle`, with where
/// that went: steps of at least `least` and what their children don't
/// account for
pub fn end(least: Duration, idle: Duration) -> Option<(Duration, String)> {
    let mut pass = PASS.with(|pass| pass.borrow_mut().take())?;
    pass.nodes[0].took = pass.began.elapsed().saturating_sub(idle);
    let mut out = String::new();
    write_node(&pass.nodes, 0, least, &mut out);
    Some((pass.nodes[0].took, out))
}

fn write_node(nodes: &[Node], index: usize, least: Duration, out: &mut String) {
    let node = &nodes[index];
    let ms = |took: Duration| took.as_secs_f64() * 1000.0;
    let _ = write!(out, "{} {:.0}", node.name, ms(node.took));
    if node.count > 1 {
        let _ = write!(out, "x{}", node.count);
    }
    let mut children: Vec<usize> = node.children.iter().copied().filter(|&child| nodes[child].took >= least).collect();
    if children.is_empty() {
        return;
    }
    children.sort_by_key(|&child| std::cmp::Reverse(nodes[child].took));
    let counted: Duration = node.children.iter().map(|&child| nodes[child].took).sum();
    out.push_str(" [");
    for (at, &child) in children.iter().enumerate() {
        if at > 0 {
            out.push_str(", ");
        }
        write_node(nodes, child, least, out);
    }
    // Time spent in this step itself or in steps not timed
    let rest = node.took.saturating_sub(counted);
    if rest >= least {
        let _ = write!(out, ", untimed {:.0}", ms(rest));
    }
    out.push(']');
}
