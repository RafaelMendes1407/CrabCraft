//! The tree of the Hierarchy window: who calls a function, or what it calls. Children load when a
//! node opens, so recursive functions cost nothing until the user follows them.

use crate::insight::Target;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct CallNode {
    pub name: String,
    /// The function's signature, as the server writes it.
    pub detail: String,
    /// Where a click goes: the call for callers, the definition for calls.
    pub target: Option<Target>,
    /// How many times the parent's call appears in this function.
    pub calls: usize,
    /// The server's item, to ask for the next level.
    pub item: Value,
    /// `None` until loaded.
    pub children: Option<Vec<CallNode>>,
    pub open: bool,
}

impl CallNode {
    pub fn new(item: Value, target: Option<Target>, calls: usize) -> Self {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        CallNode { name: text("name"), detail: text("detail"), target, calls, item, children: None, open: false }
    }
}

#[derive(Clone, Debug)]
pub struct CallTree {
    pub callers: bool,
    pub root: CallNode,
}

impl CallTree {
    /// The node at `path`, a list of child indices from the root.
    pub fn node_mut(&mut self, path: &[usize]) -> Option<&mut CallNode> {
        let mut node = &mut self.root;
        for &i in path {
            node = node.children.as_mut()?.get_mut(i)?;
        }
        Some(node)
    }

    /// The visible nodes in order, with their paths.
    pub fn rows(&self) -> Vec<(Vec<usize>, &CallNode)> {
        fn walk<'a>(node: &'a CallNode, path: Vec<usize>, out: &mut Vec<(Vec<usize>, &'a CallNode)>) {
            out.push((path.clone(), node));
            if let (true, Some(children)) = (node.open, &node.children) {
                for (i, child) in children.iter().enumerate() {
                    let mut next = path.clone();
                    next.push(i);
                    walk(child, next, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, Vec::new(), &mut out);
        out
    }
}

/// The server's item, range and call count of each entry of an incoming or outgoing reply.
pub fn entries(reply: &Value, callers: bool) -> Vec<(Value, Value, usize)> {
    let calls = reply.as_array().map_or(&[][..], Vec::as_slice);
    let mut out = Vec::new();
    for call in calls {
        let Some(item) = call.get(if callers { "from" } else { "to" }) else { continue };
        let ranges = call.get("fromRanges").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        let place = if callers { ranges.first() } else { item.get("selectionRange") };
        if let Some(place) = place {
            out.push((item.clone(), place.clone(), ranges.len().max(1)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(name: &str, children: Option<Vec<CallNode>>, open: bool) -> CallNode {
        let mut n = CallNode::new(json!({ "name": name }), None, 1);
        n.children = children;
        n.open = open;
        n
    }

    #[test]
    fn closed_nodes_hide_their_children() {
        let inner = node("c", Some(vec![node("d", None, false)]), false);
        let mut tree = CallTree { callers: true, root: node("a", Some(vec![node("b", None, false), inner]), true) };
        let names = |t: &CallTree| t.rows().iter().map(|(p, n)| (p.len(), n.name.clone())).collect::<Vec<_>>();
        assert_eq!(names(&tree), [(0, "a".to_string()), (1, "b".to_string()), (1, "c".to_string())]);
        tree.node_mut(&[1]).unwrap().open = true;
        assert_eq!(names(&tree).last(), Some(&(2, "d".to_string())));
        assert!(tree.node_mut(&[0, 7]).is_none());
    }

    #[test]
    fn replies_keep_the_first_call_and_count_the_rest() {
        let reply = json!([{ "from": { "name": "f" }, "fromRanges": [{ "n": 1 }, { "n": 2 }] }, { "from": { "name": "no calls" }, "fromRanges": [] }]);
        assert_eq!(entries(&reply, true), [(json!({ "name": "f" }), json!({ "n": 1 }), 2)]);
        let reply = json!([{ "to": { "name": "g", "selectionRange": { "n": 3 } }, "fromRanges": [{ "n": 4 }] }]);
        assert_eq!(entries(&reply, false)[0].1, json!({ "n": 3 }));
        assert!(entries(&Value::Null, true).is_empty());
    }
}
