use crate::{CoreError, ProtocolSession, input::Point};
use serde::Deserialize;
use serde_json::{Value, json};

const BOUNDS_JS: &str = include_str!("../assets/geometry.js");

#[derive(Debug, Deserialize)]
struct QuadsResult {
    quads: Option<Vec<Vec<f64>>>,
}

#[derive(Debug, Deserialize)]
struct BoxModelResult {
    model: Option<BoxModel>,
}

#[derive(Debug, Deserialize)]
struct BoxModel {
    content: Vec<f64>,
}

#[derive(Debug, Deserialize)]
struct ResolveNodeResult {
    object: Option<RemoteObject>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteObject {
    object_id: Option<String>,
    value: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CallFunctionResult {
    result: RemoteObject,
}

#[derive(Debug, Deserialize)]
struct PushNodesResult {
    #[serde(rename = "nodeIds")]
    node_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct Bounds {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

pub async fn get_element_center(
    session: &ProtocolSession,
    backend_node_id: i64,
) -> Result<Point, CoreError> {
    let quads = session
        .send::<_, QuadsResult>(
            "DOM.getContentQuads",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await;
    if let Ok(quads) = quads
        && let Some(quad) = quads.quads.and_then(|quads| quads.into_iter().next())
        && quad.len() >= 8
    {
        return Ok(quad_center(&quad));
    }

    let model = session
        .send::<_, BoxModelResult>(
            "DOM.getBoxModel",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await;
    if let Ok(model) = model
        && let Some(model) = model.model
        && model.content.len() >= 8
    {
        return Ok(quad_center(&model.content));
    }

    let object_id = resolve_object_id(session, backend_node_id, None).await?;
    let bounds: CallFunctionResult = session
        .send(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": BOUNDS_JS,
                "objectId": object_id,
                "returnByValue": true
            }),
        )
        .await?;
    let value = bounds
        .result
        .value
        .ok_or_else(|| CoreError::Message("Could not get element bounds.".to_string()))?;
    let rect: Bounds =
        serde_json::from_value(value).map_err(|err| CoreError::Message(err.to_string()))?;
    Ok(Point {
        x: rect.x + rect.w / 2.0,
        y: rect.y + rect.h / 2.0,
    })
}

pub async fn scroll_into_view(session: &ProtocolSession, backend_node_id: i64) {
    let _ = session
        .send::<_, Value>(
            "DOM.scrollIntoViewIfNeeded",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await;
}

pub async fn focus_element(
    session: &ProtocolSession,
    backend_node_id: i64,
) -> Result<(), CoreError> {
    let pushed: PushNodesResult = session
        .send(
            "DOM.pushNodesByBackendIdsToFrontend",
            json!({ "backendNodeIds": [backend_node_id] }),
        )
        .await?;
    let Some(node_id) = pushed.node_ids.first() else {
        return Err(CoreError::Message(
            "Element not found in DOM. Take a new snapshot.".to_string(),
        ));
    };
    let _: Value = session
        .send("DOM.focus", json!({ "nodeId": node_id }))
        .await?;
    Ok(())
}

pub async fn js_click(session: &ProtocolSession, backend_node_id: i64) -> Result<(), CoreError> {
    let object_id = resolve_object_id(session, backend_node_id, None).await?;
    let _: Value = session
        .send(
            "Runtime.callFunctionOn",
            json!({ "functionDeclaration": "function(){this.click()}", "objectId": object_id }),
        )
        .await?;
    Ok(())
}

pub async fn get_input_value(session: &ProtocolSession, backend_node_id: i64) -> String {
    call_on_element(
        session,
        backend_node_id,
        "function(){return this.value??this.textContent??\"\"}",
        None,
    )
    .await
    .ok()
    .and_then(|value| value.as_str().map(ToString::to_string))
    .unwrap_or_default()
}

pub async fn call_on_element(
    session: &ProtocolSession,
    backend_node_id: i64,
    function_declaration: &str,
    args: Option<Vec<Value>>,
) -> Result<Value, CoreError> {
    let object_id = resolve_object_id(session, backend_node_id, None).await?;
    let arguments = args.map(|args| {
        args.into_iter()
            .map(|value| json!({ "value": value }))
            .collect::<Vec<_>>()
    });
    let result: CallFunctionResult = session
        .send(
            "Runtime.callFunctionOn",
            json!({
                "functionDeclaration": function_declaration,
                "objectId": object_id,
                "returnByValue": true,
                "arguments": arguments
            }),
        )
        .await?;
    Ok(result.result.value.unwrap_or(Value::Null))
}

pub async fn resolve_object_id(
    session: &ProtocolSession,
    backend_node_id: i64,
    object_group: Option<&str>,
) -> Result<String, CoreError> {
    let mut params = serde_json::Map::new();
    params.insert("backendNodeId".to_string(), Value::from(backend_node_id));
    if let Some(object_group) = object_group {
        params.insert(
            "objectGroup".to_string(),
            Value::String(object_group.to_string()),
        );
    }
    let resolved: ResolveNodeResult = session
        .send("DOM.resolveNode", Value::Object(params))
        .await?;
    resolved
        .object
        .and_then(|object| object.object_id)
        .ok_or_else(|| {
            CoreError::Message("Element not found in DOM. Take a new snapshot.".to_string())
        })
}

fn quad_center(quad: &[f64]) -> Point {
    Point {
        x: (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0,
        y: (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0,
    }
}
