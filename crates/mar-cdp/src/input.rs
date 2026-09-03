//! Synthetic input, and the coordinates that make it possible.
//!
//! A CDP client does not click a selector: it reads an element's rectangle,
//! takes the centre, and sends the coordinates to `Input.dispatchMouseEvent`.
//! Without layout every rectangle is the same zero-sized box at the origin, so
//! every click would mean the same thing — nothing.
//!
//! The page's prelude answers geometry questions from a synthetic spatial
//! index instead: each element that is asked about gets its own tile in an
//! imaginary grid, and the grid remembers which tile belongs to which element.
//! Geometry methods hand those tiles out, input events look elements up by
//! coordinate, and a click that started as a rectangle ends on the element it
//! came from. Nothing is laid out; the tiles say only "this element, and not
//! that one".
//!
//! Only the client is answered this way. Tiles exist while the client is
//! measuring — `DOM.getContentQuads` here, and any `Runtime.evaluate`, which
//! is how a modern Puppeteer measures — and the page's own scripts go on
//! seeing the zero-sized box at the origin they see with no client attached.

use crate::browser::Target;
use crate::protocol::Command;
use mar_dom::NodeId;
use serde_json::{Value, json};

/// A tile, as the page reported it.
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// CDP quads run clockwise from the top left, eight numbers to a box.
    pub fn quad(&self) -> Vec<f64> {
        vec![
            self.x,
            self.y,
            self.x + self.width,
            self.y,
            self.x + self.width,
            self.y + self.height,
            self.x,
            self.y + self.height,
        ]
    }
}

/// A JS string literal for a value that came from the client.
fn quoted(text: &str) -> String {
    // JSON string syntax is JS string syntax, and it is already escaped.
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

fn call(target: &mut Target, expression: &str) -> Result<Value, String> {
    let page = target
        .page
        .as_mut()
        .ok_or_else(|| "no page: navigate first".to_owned())?;
    let json = page.eval_json(expression)?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

/// The tile a node holds, assigning one if this is the first time it is asked
/// about. Absent when the node is gone from the tree.
pub fn rect_of(target: &mut Target, node: NodeId) -> Result<Option<Rect>, String> {
    let value = call(target, &format!("__mar_layout_rect({})", node.as_u32()))?;
    Ok(value.as_object().map(|o| {
        let read = |name: &str| o.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        Rect {
            x: read("x"),
            y: read("y"),
            width: read("width"),
            height: read("height"),
        }
    }))
}

/// How far the tiles handed out so far reach.
///
/// A client clamps a click point to the layout viewport it was told about and
/// drops the click if that leaves no area, so `Page.getLayoutMetrics` has to
/// report something at least this big.
pub fn extent(target: &mut Target) -> Option<(u32, u32)> {
    let value = call(target, "__mar_layout_extent()").ok()?;
    let list = value.as_array()?;
    Some((
        list.first()?.as_f64()? as u32,
        list.get(1)?.as_f64()? as u32,
    ))
}

pub fn dispatch_mouse(target: &mut Target, command: &Command) -> Result<Value, String> {
    let kind = command.str_param("type").unwrap_or("mouseMoved");
    let x = command
        .params
        .get("x")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let y = command
        .params
        .get("y")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let button = command.str_param("button").unwrap_or("none");
    let clicks = command.int_param("clickCount").unwrap_or(0);
    let modifiers = command.int_param("modifiers").unwrap_or(0);

    let hit = call(
        target,
        &format!(
            "__mar_input_mouse({}, {x}, {y}, {}, {clicks}, {modifiers})",
            quoted(kind),
            quoted(button)
        ),
    )?;
    // Evaluation can move the DOM under the cached document, and a click is
    // usually meant to.
    target.refresh();

    // A point on no tile is not an error: Chrome dispatches into empty space
    // and says nothing. It is worth a line in the log, because it is what a
    // client sees when it clicks a rectangle it never asked for.
    if hit.as_u64() == Some(0) {
        tracing::debug!(x, y, "mouse event landed on no element");
    }
    Ok(json!({}))
}

pub fn dispatch_key(target: &mut Target, command: &Command) -> Result<Value, String> {
    let kind = command.str_param("type").unwrap_or("keyDown");
    let key = command.str_param("key").unwrap_or("");
    let code = command.str_param("code").unwrap_or("");
    // `text` is what the key produces; a bare arrow or modifier key has none.
    let text = command
        .str_param("text")
        .or_else(|| command.str_param("unmodifiedText"))
        .unwrap_or("");
    let modifiers = command.int_param("modifiers").unwrap_or(0);

    call(
        target,
        &format!(
            "__mar_input_key({}, {}, {}, {}, {modifiers})",
            quoted(kind),
            quoted(key),
            quoted(code),
            quoted(text)
        ),
    )?;
    target.refresh();
    Ok(json!({}))
}

/// `DOM.focus`, which decides where the next key event lands.
pub fn focus(target: &mut Target, node: NodeId) -> Result<(), String> {
    call(
        target,
        &format!(
            "(function (n) {{ if (n) n.focus(); return !!n; }})(__mar_node_by_id({}))",
            node.as_u32()
        ),
    )?;
    Ok(())
}

/// `Input.insertText`, which puts text in without pretending keys were pressed.
pub fn insert_text(target: &mut Target, command: &Command) -> Result<Value, String> {
    let text = command.str_param("text").unwrap_or("");
    call(
        target,
        &format!(
            "__mar_input_key(\"insertText\", \"\", \"\", {}, 0)",
            quoted(text)
        ),
    )?;
    target.refresh();
    Ok(json!({}))
}
