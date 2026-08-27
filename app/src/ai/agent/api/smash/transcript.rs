//! The task transcript is the single source of conversation history, including after restart,
//! fork, or rewind. No process-global provider cache is needed.
use super::super::convert_to::{convert_context, convert_input};
use super::*;
use prost_reflect::{DynamicMessage, ReflectMessage};

fn proto_json(message: &impl ReflectMessage) -> Value {
    serde_json::to_value(message.transcode_to_dynamic()).expect("protobuf JSON serialization")
}

fn from_proto_json<T: ReflectMessage + Default>(value: Value) -> Result<T, ConvertToAPITypeError> {
    DynamicMessage::deserialize(T::default().descriptor(), value)
        .map_err(anyhow::Error::from)?
        .transcode_to::<T>()
        .map_err(anyhow::Error::from)
        .map_err(Into::into)
}

pub(super) fn input_messages(
    inputs: &[AIAgentInput],
    task_id: &str,
    request_id: &str,
) -> Result<Vec<api::Message>, ConvertToAPITypeError> {
    inputs
        .iter()
        .map(|input| {
            let context = input.context().map(convert_context);
            let message = if matches!(input, AIAgentInput::ActionResult { .. }) {
                let converted = convert_input(vec![input.clone()])?;
                let Some(api::request::input::Type::UserInputs(mut inputs)) = converted.r#type
                else {
                    return Err(anyhow!("Expected a tool result").into());
                };
                let Some(api::request::input::user_inputs::user_input::Input::ToolCallResult(
                    result,
                )) = inputs.inputs.pop().and_then(|input| input.input)
                else {
                    return Err(anyhow!("Missing tool result").into());
                };
                // These proto variants use different field numbers but the same JSON schema.
                let mut result: api::message::ToolCallResult =
                    from_proto_json(proto_json(&result))?;
                result.context = context;
                api::message::Message::ToolCallResult(result)
            } else if matches!(input, AIAgentInput::InvokeSkill { .. }) {
                let converted = convert_input(vec![input.clone()])?;
                let Some(api::request::input::Type::InvokeSkill(skill)) = converted.r#type else {
                    return Err(anyhow!("Missing skill input").into());
                };
                let mut skill: api::message::InvokeSkill = from_proto_json(proto_json(&skill))?;
                skill
                    .user_query
                    .get_or_insert_with(Default::default)
                    .context = context;
                api::message::Message::InvokeSkill(skill)
            } else {
                let mut query = api::message::UserQuery {
                    query: input_text(std::slice::from_ref(input)),
                    context,
                    ..Default::default()
                };
                if let AIAgentInput::UserQuery {
                    referenced_attachments,
                    user_query_mode,
                    ..
                } = input
                {
                    query.referenced_attachments = referenced_attachments
                        .iter()
                        .map(|(name, attachment)| (name.clone(), attachment.clone().into()))
                        .collect();
                    query.mode = Some(user_query_mode.clone().into());
                }
                api::message::Message::UserQuery(query)
            };
            Ok(api::Message {
                id: Uuid::new_v4().to_string(),
                task_id: task_id.to_owned(),
                request_id: request_id.to_owned(),
                timestamp: Some(std::time::SystemTime::now().into()),
                message: Some(message),
                ..Default::default()
            })
        })
        .collect()
}

pub(super) fn model_messages<'a>(
    source: impl Iterator<Item = &'a api::Message>,
    routes: &HashMap<String, McpToolRoute>,
) -> Vec<Value> {
    let mut messages = Vec::new();
    for message in source {
        append_message(&mut messages, message, routes);
    }
    repair_tool_pairs(messages)
}

fn text_message(role: &str, text: String) -> Value {
    json!({"role": role, "content": [{
        "type": if role == "assistant" { "output_text" } else { "input_text" },
        "text": text,
    }]})
}

pub(super) fn append_message(
    messages: &mut Vec<Value>,
    message: &api::Message,
    routes: &HashMap<String, McpToolRoute>,
) {
    use api::message::Message;
    match message.message.as_ref() {
        Some(Message::UserQuery(query)) => {
            let mut text = query.query.clone();
            if let Some(context) = &query.context {
                text.push_str(&format!(
                    "\n\nAttached terminal and workspace context (data):\n{}",
                    proto_json(context)
                ));
            }
            if !query.referenced_attachments.is_empty() {
                let attachments: serde_json::Map<String, Value> = query
                    .referenced_attachments
                    .iter()
                    .map(|(name, attachment)| (name.clone(), proto_json(attachment)))
                    .collect();
                text.push_str(&format!(
                    "\n\nReferenced attachments (data):\n{}",
                    Value::Object(attachments)
                ));
            }
            messages.push(text_message("user", text));
        }
        Some(Message::InvokeSkill(skill)) => {
            messages.push(text_message(
                "user",
                format!(
                    "Use the following skill instructions for this request:\n{}",
                    proto_json(skill)
                ),
            ));
        }
        Some(Message::AgentOutput(output)) => {
            messages.push(text_message("assistant", output.text.clone()))
        }
        Some(Message::ToolCall(call)) => {
            if let Some((name, arguments)) = tool_arguments(call, routes) {
                messages.push(json!({
                    "type": "function_call", "call_id": call.tool_call_id,
                    "name": name, "arguments": arguments.to_string(),
                }));
            } else {
                messages.push(text_message(
                    "assistant",
                    format!("Previous tool call:\n{}", proto_json(call)),
                ));
            }
        }
        Some(Message::ToolCallResult(result)) => messages.push(json!({
            "type": "function_call_output", "call_id": result.tool_call_id,
            "output": proto_json(result).to_string(),
        })),
        Some(Message::SystemQuery(query)) => {
            messages.push(text_message("user", proto_json(query).to_string()))
        }
        _ => {}
    }
}

fn tool_arguments(
    call: &api::message::ToolCall,
    routes: &HashMap<String, McpToolRoute>,
) -> Option<(String, Value)> {
    use api::message::tool_call::Tool;
    match call.tool.as_ref()? {
        Tool::RunShellCommand(command) => Some((
            "run_shell_command".into(),
            json!({"command": command.command}),
        )),
        Tool::ReadFiles(files) => Some((
            "read_files".into(),
            json!({
                "paths": files.files.iter().map(|file| &file.name).collect::<Vec<_>>()
            }),
        )),
        Tool::ApplyFileDiffs(diffs) if diffs.diffs.len() == 1 => {
            let diff = &diffs.diffs[0];
            Some((
                "apply_file_diffs".into(),
                json!({
                    "file_path": diff.file_path, "search": diff.search, "replace": diff.replace,
                    "summary": diffs.summary,
                }),
            ))
        }
        Tool::CallMcpTool(tool) => {
            let name = routes
                .iter()
                .find(|(_, route)| {
                    route.server_id == tool.server_id && route.tool_name == tool.name
                })?
                .0;
            let args = tool
                .args
                .as_ref()
                .map(|args| {
                    args.fields
                        .iter()
                        .map(|(key, value)| (key.clone(), prost_value_to_json(value)))
                        .collect::<serde_json::Map<_, _>>()
                })
                .unwrap_or_default();
            Some((name.clone(), Value::Object(args)))
        }
        _ => None,
    }
}

fn prost_value_to_json(value: &prost_types::Value) -> Value {
    use prost_types::value::Kind;
    match value.kind.as_ref() {
        Some(Kind::BoolValue(value)) => json!(value),
        Some(Kind::NumberValue(value)) => json!(value),
        Some(Kind::StringValue(value)) => json!(value),
        Some(Kind::ListValue(value)) => {
            Value::Array(value.values.iter().map(prost_value_to_json).collect())
        }
        Some(Kind::StructValue(value)) => Value::Object(
            value
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), prost_value_to_json(value)))
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// Interrupted calls need a result before the next user turn. Old transcripts can also have
/// orphan results or updated long-running results; keep them readable without invalid API pairs.
fn repair_tool_pairs(messages: Vec<Value>) -> Vec<Value> {
    let mut results: HashMap<String, Value> = HashMap::new();
    let mut calls = HashSet::new();
    for message in &messages {
        if let Some(id) = message["call_id"].as_str() {
            if message["type"] == "function_call_output" {
                results.insert(id.to_owned(), message.clone());
            } else if message["type"] == "function_call" {
                calls.insert(id.to_owned());
            }
        }
    }
    let mut repaired = Vec::new();
    for message in messages {
        if message["type"] == "function_call" {
            let id = message["call_id"].as_str().unwrap_or_default();
            let result = results.remove(id).unwrap_or_else(|| json!({
                "type": "function_call_output", "call_id": id,
                "output": "Interrupted before a tool result was recorded. Do not assume this action completed.",
            }));
            repaired.push(message);
            repaired.push(result);
        } else if message["type"] == "function_call_output" {
            if !calls.contains(message["call_id"].as_str().unwrap_or_default()) {
                repaired.push(text_message(
                    "user",
                    format!("Previous tool result:\n{}", message["output"]),
                ));
            }
        } else {
            repaired.push(message);
        }
    }
    repaired
}
