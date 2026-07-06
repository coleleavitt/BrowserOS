use std::{collections::BTreeMap, env, fs, path::Path};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Protocol {
    domains: Vec<Domain>,
}

#[derive(Debug, Deserialize)]
struct Domain {
    domain: String,
    #[serde(default)]
    types: Vec<TypeDef>,
    #[serde(default)]
    commands: Vec<Command>,
    #[serde(default)]
    events: Vec<Event>,
}

#[derive(Debug, Deserialize)]
struct TypeDef {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    properties: Vec<Property>,
    #[serde(default)]
    items: Option<Item>,
}

#[derive(Debug, Deserialize)]
struct Command {
    name: String,
    #[serde(default)]
    parameters: Vec<Property>,
    #[serde(default)]
    returns: Vec<Property>,
}

#[derive(Debug, Deserialize)]
struct Event {
    name: String,
    #[serde(default)]
    parameters: Vec<Property>,
}

#[derive(Debug, Clone, Deserialize)]
struct Property {
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "$ref")]
    ref_name: Option<String>,
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    items: Option<Item>,
}

#[derive(Debug, Clone, Deserialize)]
struct Item {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "$ref")]
    ref_name: Option<String>,
    #[serde(default)]
    items: Option<Box<Item>>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let protocol_dir = Path::new(&manifest_dir).join("protocol");
    let files = [
        "browser_protocol.json",
        "js_protocol.json",
        "browseros_protocol.json",
    ];

    let mut domains = BTreeMap::<String, Domain>::new();
    for file in files {
        let path = protocol_dir.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let json = fs::read_to_string(&path)?;
        let protocol: Protocol = serde_json::from_str(&json)?;
        for domain in protocol.domains {
            domains.insert(domain.domain.clone(), domain);
        }
    }

    let mut out = String::from(
        "use serde::{Deserialize, Serialize};\n\
         use serde_json::Value;\n\
         use crate::{CdpClient, CdpError, SessionId};\n\n\
         #[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]\n\
         pub struct EmptyParams {}\n\n",
    );

    for domain in domains.values() {
        emit_domain(&mut out, domain);
    }

    let out_dir = env::var("OUT_DIR")?;
    fs::write(Path::new(&out_dir).join("protocol.rs"), out)?;
    Ok(())
}

fn emit_domain(out: &mut String, domain: &Domain) {
    let module = to_snake(&domain.domain);
    out.push_str(&format!("pub mod {module} {{\nuse super::*;\n\n"));

    for typedef in &domain.types {
        emit_type(out, domain, typedef);
    }
    for command in &domain.commands {
        emit_command_types(out, domain, command);
    }
    for event in &domain.events {
        emit_event_type(out, domain, event);
    }
    for command in &domain.commands {
        emit_command_fn(out, domain, command);
    }

    out.push_str("}\n\n");
}

fn emit_type(out: &mut String, domain: &Domain, typedef: &TypeDef) {
    let name = to_pascal(&typedef.id);
    match typedef.kind.as_str() {
        "object" => emit_struct(out, domain, &name, &typedef.id, &typedef.properties),
        "array" => {
            let item_ty = typedef
                .items
                .as_ref()
                .map(|item| rust_item_type(&domain.domain, &typedef.id, "", item))
                .unwrap_or_else(|| "Value".to_string());
            out.push_str(&format!("pub type {name} = Vec<{item_ty}>;\n\n"));
        }
        _ => {
            let ty = rust_primitive(&typedef.kind);
            out.push_str(&format!("pub type {name} = {ty};\n\n"));
        }
    }
}

fn emit_command_types(out: &mut String, domain: &Domain, command: &Command) {
    let base = to_pascal(&command.name);
    let params = format!("{base}Params");
    let result = format!("{base}Result");
    if !command.parameters.is_empty() {
        emit_struct(out, domain, &params, &params, &command.parameters);
    }
    emit_struct(out, domain, &result, &result, &command.returns);
}

fn emit_event_type(out: &mut String, domain: &Domain, event: &Event) {
    let name = format!("{}Event", to_pascal(&event.name));
    emit_struct(out, domain, &name, &name, &event.parameters);
}

fn emit_struct(
    out: &mut String,
    domain: &Domain,
    rust_name: &str,
    type_name: &str,
    properties: &[Property],
) {
    out.push_str("#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]\n");
    out.push_str(&format!("pub struct {rust_name} {{\n"));
    for property in properties {
        let field = field_name(&property.name);
        let ty = rust_property_type(&domain.domain, type_name, property);
        if property.optional {
            out.push_str(&format!(
                "    #[serde(rename = \"{}\", skip_serializing_if = \"Option::is_none\")]\n",
                property.name
            ));
        } else {
            out.push_str(&format!("    #[serde(rename = \"{}\")]\n", property.name));
        }
        out.push_str(&format!("    pub {field}: {ty},\n"));
    }
    out.push_str("}\n\n");
}

fn emit_command_fn(out: &mut String, domain: &Domain, command: &Command) {
    let fn_name = to_snake(&command.name);
    let method = format!("{}.{}", domain.domain, command.name);
    let base = to_pascal(&command.name);
    let params = if command.parameters.is_empty() {
        "EmptyParams".to_string()
    } else {
        format!("{base}Params")
    };
    let result = format!("{base}Result");
    let args = if command.parameters.is_empty() {
        "EmptyParams {}".to_string()
    } else {
        "params".to_string()
    };
    let params_arg = if command.parameters.is_empty() {
        String::new()
    } else {
        format!("params: {params}, ")
    };

    out.push_str(&format!(
        "pub async fn {fn_name}(client: &CdpClient, {params_arg}session: Option<&SessionId>) -> Result<{result}, CdpError> {{\n    client.send_typed(\"{method}\", {args}, session).await\n}}\n\n"
    ));
}

fn rust_property_type(domain: &str, type_name: &str, property: &Property) -> String {
    let mut ty = if let Some(ref_name) = &property.ref_name {
        rust_ref_type(domain, ref_name)
    } else if property.kind.as_deref() == Some("array") {
        property
            .items
            .as_ref()
            .map(|item| {
                format!(
                    "Vec<{}>",
                    rust_item_type(domain, type_name, &property.name, item)
                )
            })
            .unwrap_or_else(|| "Vec<Value>".to_string())
    } else {
        rust_primitive(property.kind.as_deref().unwrap_or("object")).to_string()
    };

    if is_boxed(domain, type_name, &property.name) {
        ty = format!("Box<{ty}>");
    }
    if property.optional {
        ty = format!("Option<{ty}>");
    }
    ty
}

fn rust_item_type(domain: &str, type_name: &str, property_name: &str, item: &Item) -> String {
    if let Some(ref_name) = &item.ref_name {
        let mut ty = rust_ref_type(domain, ref_name);
        if is_boxed(domain, type_name, property_name) {
            ty = format!("Box<{ty}>");
        }
        return ty;
    }
    if item.kind.as_deref() == Some("array") {
        let inner = item
            .items
            .as_deref()
            .map(|inner| rust_item_type(domain, type_name, property_name, inner))
            .unwrap_or_else(|| "Value".to_string());
        return format!("Vec<{inner}>");
    }
    rust_primitive(item.kind.as_deref().unwrap_or("object")).to_string()
}

fn rust_ref_type(current_domain: &str, ref_name: &str) -> String {
    if let Some((domain, name)) = ref_name.split_once('.') {
        let module = to_snake(domain);
        format!("super::{module}::{}", to_pascal(name))
    } else {
        let _ = current_domain;
        to_pascal(ref_name)
    }
}

fn rust_primitive(kind: &str) -> &'static str {
    match kind {
        "string" => "String",
        "integer" => "i64",
        "number" => "f64",
        "boolean" => "bool",
        "object" => "Value",
        _ => "Value",
    }
}

fn is_boxed(domain: &str, type_name: &str, property_name: &str) -> bool {
    matches!(
        (domain, type_name, property_name),
        ("DOM", "Node", "contentDocument") | ("Runtime", "StackTrace", "parent")
    )
}

fn to_pascal(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::new();
    out.extend(first.to_uppercase());
    out.push_str(chars.as_str());
    out
}

fn to_snake(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::new();
    for (index, ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase() {
            let prev = index.checked_sub(1).and_then(|prev| chars.get(prev));
            let next = chars.get(index + 1);
            let boundary = prev.is_some_and(|prev| {
                prev.is_ascii_lowercase()
                    || prev.is_ascii_digit()
                    || next.is_some_and(|next| next.is_ascii_lowercase())
            });
            if index > 0 && boundary {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if *ch == '-' || *ch == ' ' {
            out.push('_');
        } else {
            out.push(*ch);
        }
    }
    out
}

fn field_name(value: &str) -> String {
    let name = to_snake(value);
    match name.as_str() {
        "type" => "r#type".to_string(),
        "ref" => "r#ref".to_string(),
        _ => name,
    }
}
