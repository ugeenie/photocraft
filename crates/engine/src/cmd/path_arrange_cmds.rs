//! Stacking order of path components: Photoshop's Path Arrangement buttons for the Path
//! Selection tool (Bring to Front / Bring Forward / Send Backward / Send to Back) reorder the
//! components of a shape layer's path, a vector mask, the work path or a saved path.
//! `path.arrange` moves the addressed components in one undoable step; a multi-selection keeps
//! its internal order and swaps as a group with the next unselected component.

use photocraft_doc::{Path, Subpath};
use serde_json::Value;

use crate::commands::CommandSpec;
use crate::vector_cmds::{bad, has_doc, leak};
use crate::{Result, Session};

/// `by` and its history-step label, as in Photoshop's options-bar buttons.
fn by_and_label(p: &Value) -> Result<(&'static str, &'static str)> {
    const CMD: &str = "path.arrange";
    match p.get("by").and_then(Value::as_str) {
        Some("front") => Ok(("front", "Bring to Front")),
        Some("forward") => Ok(("forward", "Bring Forward")),
        Some("backward") => Ok(("backward", "Send Backward")),
        Some("back") => Ok(("back", "Send to Back")),
        _ => Err(bad(CMD, "`by` must be \"front\" | \"forward\" | \"backward\" | \"back\"")),
    }
}

/// `p`'s `subpaths` param: a non-empty list of distinct indices addressing `path`.
fn subpath_indices(cmd: &str, p: &Value, path: &Path) -> Result<Vec<usize>> {
    let list = p.get("subpaths").and_then(Value::as_array).filter(|a| !a.is_empty()).ok_or_else(|| bad(cmd, "`subpaths` must list subpath indices"))?;
    let mut out: Vec<usize> = Vec::with_capacity(list.len());
    for v in list {
        let i = v.as_u64().and_then(|u| usize::try_from(u).ok()).ok_or_else(|| bad(cmd, "each `subpaths` entry is a subpath index"))?;
        if path.subpaths.get(i).is_none() {
            return Err(bad(cmd, format!("no subpath {i}")));
        }
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// Removes the subpaths at `sel` (sorted, distinct, valid) in one pass, in their original order.
fn take(path: &mut Path, sel: &[usize]) -> Vec<Subpath> {
    sel.iter().rev().map(|&i| path.subpaths.remove(i)).collect::<Vec<_>>().into_iter().rev().collect()
}

/// Moves the subpaths at `sel` so the group keeps its relative order and lands where `by`
/// dictates: `front`/`back` to the end/start, `forward`/`backward` swap it with the next
/// unselected component. Errors without touching the path when already at the boundary.
fn arrange(path: &mut Path, sel: &[usize], by: &str) -> Result<()> {
    const CMD: &str = "path.arrange";
    let n = path.subpaths.len();
    let before_anchor = |anchor: usize| sel.iter().filter(|&&i| i < anchor).count();
    match by {
        "front" => {
            let taken = take(path, sel);
            path.subpaths.extend(taken);
        }
        "back" => {
            let taken = take(path, sel);
            path.subpaths.splice(0..0, taken);
        }
        "forward" => {
            let Some(anchor) = (sel.last().copied().unwrap_or(0) + 1..n).find(|i| !sel.contains(i)) else {
                return Err(bad(CMD, "the components can't move further"));
            };
            let taken = take(path, sel);
            path.subpaths.splice(anchor - before_anchor(anchor) + 1..anchor - before_anchor(anchor) + 1, taken);
        }
        "backward" => {
            let Some(anchor) = (0..sel.first().copied().unwrap_or(0)).rev().find(|i| !sel.contains(i)) else {
                return Err(bad(CMD, "the components can't move further"));
            };
            let taken = take(path, sel);
            path.subpaths.splice(anchor - before_anchor(anchor)..anchor - before_anchor(anchor), taken);
        }
        _ => return Err(bad(CMD, "unknown `by`")),
    }
    Ok(())
}

fn path_arrange(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "path.arrange";
    let (by, label) = by_and_label(p)?;
    crate::path_edit_cmds::edit_path(s, CMD, p, label, |path| {
        let sel = subpath_indices(CMD, p, path).map_err(|e| e.to_string())?;
        arrange(path, &sel, by).map_err(|e| e.to_string())
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "path.arrange",
        label: "Arrange Path Components",
        menu: &[],
        shortcut: None,
        params: leak(
            r#"{"name":"work"|"layer"|saved-name="work","layer":id? (with "layer": the layer's shape path or vector mask),"subpaths":[i,…],"by":"front"|"forward"|"backward"|"back"} → {name,path} (the addressed components move as a group, keeping their order; an error at the boundary leaves the path untouched)"#.to_string(),
        ),
        enabled: has_doc,
        run: path_arrange,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use photocraft_doc::LayerContent;
    use serde_json::json;

    use crate::Session;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 200, "height": 100, "background": "white"})).unwrap();
        s
    }

    /// A shape layer with two disjoint rectangles (subpath 0 at 20..60, subpath 1 at 100..140).
    fn two_rects(s: &mut Session) -> u64 {
        let id = s.execute("shape.create", json!({"kind": "rect", "rect": [20, 20, 40, 40], "fill": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
        let k = |x: f64, y: f64| json!({"anchor": [x, y]});
        let two = json!({"subpaths": [
            {"closed": true, "knots": [k(20.0, 20.0), k(60.0, 20.0), k(60.0, 60.0), k(20.0, 60.0)]},
            {"closed": true, "knots": [k(100.0, 20.0), k(140.0, 20.0), k(140.0, 60.0), k(100.0, 60.0)]},
        ]});
        s.execute("shape.edit", json!({"layer": id, "path": two})).unwrap();
        id
    }

    /// A saved path with two disjoint rectangles.
    fn two_rects_saved(s: &mut Session) {
        let k = |x: f64, y: f64| json!({"anchor": [x, y]});
        s.execute(
            "path.set",
            json!({"name": "Sister", "path": {"subpaths": [
                {"closed": true, "knots": [k(20.0, 20.0), k(60.0, 20.0), k(60.0, 60.0), k(20.0, 60.0)]},
                {"closed": true, "knots": [k(100.0, 20.0), k(140.0, 20.0), k(140.0, 60.0), k(100.0, 60.0)]},
            ]}}),
        )
        .unwrap();
    }

    fn anchor0(s: &Session, layer: Option<u64>, name: &str) -> [photocraft_geom::Point; 2] {
        let doc = &s.active().unwrap().doc;
        let path = match layer {
            Some(id) => {
                let l = doc.layer(photocraft_doc::LayerId(id)).unwrap();
                let LayerContent::Shape(sh) = &l.content else { panic!("not a shape") };
                sh.path.clone()
            }
            None => doc.paths.iter().find(|q| q.name == name).unwrap().path.clone(),
        };
        [path.subpaths[0].knots[0].anchor, path.subpaths[1].knots[0].anchor]
    }

    #[test]
    fn bring_forward_and_send_backward_swap_single_components() {
        let mut s = session();
        let id = two_rects(&mut s);
        let base = anchor0(&s, Some(id), "");
        // [0, 1] → forward(subpaths [0]) → [1, 0]
        let r = s.execute("path.arrange", json!({"name": "layer", "layer": id, "subpaths": [0], "by": "forward"})).unwrap();
        assert_eq!(r["name"], json!("layer"));
        assert_eq!(anchor0(&s, Some(id), ""), [photocraft_geom::Point::new(100.0, 20.0), photocraft_geom::Point::new(20.0, 20.0)]);
        assert_eq!(s.active().unwrap().history.undo_label(), Some("Bring Forward"));
        assert!(s.undo());
        assert_eq!(anchor0(&s, Some(id), ""), base, "undo restores the order");
        // [0, 1] → backward(subpaths [1]) → [1, 0]
        s.execute("path.arrange", json!({"name": "layer", "layer": id, "subpaths": [1], "by": "backward"})).unwrap();
        assert_eq!(anchor0(&s, Some(id), ""), [photocraft_geom::Point::new(100.0, 20.0), photocraft_geom::Point::new(20.0, 20.0)]);
        assert_eq!(s.active().unwrap().history.undo_label(), Some("Send Backward"));
    }

    #[test]
    fn front_and_back_move_to_the_ends() {
        let mut s = session();
        let id = two_rects(&mut s);
        s.execute("path.arrange", json!({"name": "layer", "layer": id, "subpaths": [0], "by": "front"})).unwrap();
        assert_eq!(anchor0(&s, Some(id), "")[0], photocraft_geom::Point::new(100.0, 20.0), "subpath 0 is last now");
        assert_eq!(s.active().unwrap().history.undo_label(), Some("Bring to Front"));
        s.execute("path.arrange", json!({"name": "layer", "layer": id, "subpaths": [1], "by": "back"})).unwrap();
        assert_eq!(
            anchor0(&s, Some(id), ""),
            [photocraft_geom::Point::new(20.0, 20.0), photocraft_geom::Point::new(100.0, 20.0)],
            "back: subpath 1 is first now"
        );
        assert_eq!(s.active().unwrap().history.undo_label(), Some("Send to Back"));
    }

    #[test]
    fn a_group_keeps_its_order_and_swaps_as_one() {
        let mut s = session();
        let id = two_rects(&mut s);
        let k = |x: f64, y: f64| json!({"anchor": [x, y]});
        s.execute(
            "shape.edit",
            json!({"layer": id, "path": {"subpaths": [
                {"closed": true, "knots": [k(160.0, 20.0), k(180.0, 20.0), k(180.0, 40.0), k(160.0, 40.0)]},
                {"closed": true, "knots": [k(20.0, 20.0), k(60.0, 20.0), k(60.0, 60.0), k(20.0, 60.0)]},
                {"closed": true, "knots": [k(100.0, 20.0), k(140.0, 20.0), k(140.0, 60.0), k(100.0, 60.0)]},
            ]}}),
        )
        .unwrap();
        // The group {0, 1} swaps with subpath 2 as one, keeping its internal order: [2, 0, 1].
        s.execute("path.arrange", json!({"name": "layer", "layer": id, "subpaths": [0, 1], "by": "forward"})).unwrap();
        let l = s.active().unwrap().doc.layer(photocraft_doc::LayerId(id)).unwrap();
        let LayerContent::Shape(sh) = &l.content else { panic!("not a shape") };
        let anchors: Vec<_> = sh.path.subpaths.iter().map(|sp| sp.knots[0].anchor).collect();
        assert_eq!(
            anchors,
            [photocraft_geom::Point::new(100.0, 20.0), photocraft_geom::Point::new(160.0, 20.0), photocraft_geom::Point::new(20.0, 20.0),],
            "the group swapped with subpath 2, its order kept"
        );
    }

    #[test]
    fn arrange_works_on_saved_and_work_paths_by_name() {
        let mut s = session();
        two_rects_saved(&mut s);
        s.execute("path.arrange", json!({"name": "Sister", "subpaths": [0], "by": "front"})).unwrap();
        assert_eq!(anchor0(&s, None, "Sister")[0], photocraft_geom::Point::new(100.0, 20.0));
        // The work path addresses the same way.
        s.execute("path.set", json!({"name": "work", "path": {"subpaths": [{"closed": true, "knots": [[20, 20], [60, 20], [60, 60], [20, 60]]}, {"closed": true, "knots": [[100, 20], [140, 20], [140, 60], [100, 60]]}]}})).unwrap();
        s.execute("path.arrange", json!({"name": "work", "subpaths": [0], "by": "front"})).unwrap();
        let doc = &s.active().unwrap().doc;
        let work = doc.work_path.as_ref().unwrap();
        assert_eq!(work.subpaths[0].knots[0].anchor, photocraft_geom::Point::new(100.0, 20.0));
    }

    #[test]
    fn boundary_and_bad_params_fail_without_touching_the_path() {
        let mut s = session();
        let id = two_rects(&mut s);
        let before = anchor0(&s, Some(id), "");
        let steps = s.active().unwrap().history.past_len();
        for params in [
            json!({"name": "layer", "layer": id, "subpaths": [1], "by": "forward"}),
            json!({"name": "layer", "layer": id, "subpaths": [0], "by": "backward"}),
            json!({"name": "layer", "layer": id, "subpaths": [9], "by": "front"}),
            json!({"name": "layer", "layer": id, "subpaths": [], "by": "front"}),
            json!({"name": "layer", "layer": id, "subpaths": [0], "by": "sideways"}),
        ] {
            assert!(s.execute("path.arrange", params).is_err());
        }
        assert_eq!(anchor0(&s, Some(id), ""), before, "a refused arrange leaves the path alone");
        assert_eq!(s.active().unwrap().history.past_len(), steps, "no history step for a refused arrange");
    }
}
