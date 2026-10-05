use ai::skills::{ParsedSkill, SkillProvider, SkillScope};
use warp_util::local_or_remote_path::LocalOrRemotePath;

use super::*;
use crate::ai::agent::task::TaskId;
use crate::ai::agent::{AIAgentActionResult, InvokeSkillUserQuery, MCPServer};

fn action_result(id: &str) -> AIAgentInput {
    AIAgentInput::ActionResult {
        result: AIAgentActionResult {
            id: id.to_owned().into(),
            task_id: TaskId::new("task-1".to_owned()),
            result: AIAgentActionResultType::InitProject,
        },
        context: Arc::from([]),
    }
}

#[test]
fn discovered_chatgpt_models_are_forwarded_without_a_whitelist() {
    assert_eq!(normalize_model("gpt-5.6-sol"), "gpt-5.6-sol");
    assert_eq!(normalize_model("gpt-6-astra"), "gpt-6-astra");
    assert_eq!(normalize_model("oz-agent"), DEFAULT_MODEL);
}

#[test]
fn appends_every_parallel_tool_result_for_chatgpt() {
    let mut messages = Vec::new();
    append_input(
        &mut messages,
        &[action_result("call-1"), action_result("call-2")],
    );

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["type"], "function_call_output");
    assert_eq!(messages[0]["call_id"], "call-1");
    assert_eq!(messages[1]["type"], "function_call_output");
    assert_eq!(messages[1]["call_id"], "call-2");
}

#[test]
fn sends_tool_calls_to_the_action_runner_sequentially() {
    let mut output = vec![
        json!({ "type": "text", "text": "Checking." }),
        json!({ "type": "tool_use", "id": "call-1", "name": "read_files" }),
        json!({ "type": "tool_use", "id": "call-2", "name": "read_files" }),
    ];

    keep_only_first_tool_call(&mut output);

    assert_eq!(output.len(), 2);
    assert_eq!(output[1]["id"], "call-1");
}

#[test]
fn sends_skill_instructions_and_user_request_to_the_model() {
    let input = AIAgentInput::InvokeSkill {
        context: Arc::from([]),
        skill: ParsedSkill {
            path: LocalOrRemotePath::Local("/tmp/.smash/skills/fix-issue/SKILL.md".into()),
            name: "fix-issue".to_owned(),
            description: "Fix an issue".to_owned(),
            content: "Always reproduce the issue first.".to_owned(),
            line_range: None,
            provider: SkillProvider::Smash,
            scope: SkillScope::Project,
        },
        user_query: Some(InvokeSkillUserQuery {
            query: "What does this skill do?".to_owned(),
            referenced_attachments: HashMap::new(),
        }),
    };

    let text = input_text(&[input]);

    assert!(text.contains("Skill name: fix-issue"));
    assert!(text.contains("Always reproduce the issue first."));
    assert!(text.contains("What does this skill do?"));
}

fn mcp_tool(name: &str) -> rmcp::model::Tool {
    serde_json::from_value(json!({
        "name": name,
        "description": "Return a smoke-test value.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            },
            "required": ["value"]
        }
    }))
    .expect("valid MCP tool")
}

#[test]
#[allow(deprecated)]
fn includes_grouped_mcp_tools_in_every_provider_request() {
    let context = MCPContext {
        resources: vec![],
        tools: vec![],
        servers: vec![MCPServer {
            id: "31e95220-cf4e-4d48-9bd0-c8f93bf0057a".to_owned(),
            name: "Smoke server".to_owned(),
            description: String::new(),
            resources: vec![],
            tools: vec![mcp_tool("smash_mcp_probe")],
        }],
    };

    let (available_tools, routes) = tools(Some(&context));

    assert!(
        available_tools
            .iter()
            .any(|tool| tool["name"] == "smash_mcp_probe")
    );
    assert_eq!(
        routes.get("smash_mcp_probe"),
        Some(&McpToolRoute {
            server_id: "31e95220-cf4e-4d48-9bd0-c8f93bf0057a".to_owned(),
            tool_name: "smash_mcp_probe".to_owned(),
        })
    );
}

#[test]
fn converts_model_mcp_call_to_client_mcp_action() {
    let routes = HashMap::from([(
        "smash_mcp_probe".to_owned(),
        McpToolRoute {
            server_id: "31e95220-cf4e-4d48-9bd0-c8f93bf0057a".to_owned(),
            tool_name: "smash_mcp_probe".to_owned(),
        },
    )]);
    let block = json!({
        "type": "tool_use",
        "id": "call-1",
        "name": "smash_mcp_probe",
        "input": { "value": "live-test", "nested": [true, 3] }
    });

    let message = tool_call_message("task-1", "request-1", &block, &routes)
        .expect("MCP call should be converted");
    let Some(api::message::Message::ToolCall(tool_call)) = message.message else {
        panic!("expected tool call message");
    };
    let Some(api::message::tool_call::Tool::CallMcpTool(call)) = tool_call.tool else {
        panic!("expected MCP tool call");
    };
    assert_eq!(call.name, "smash_mcp_probe");
    assert_eq!(call.server_id, "31e95220-cf4e-4d48-9bd0-c8f93bf0057a");
    let args = call.args.expect("MCP args");
    assert_eq!(
        args.fields["value"].kind,
        Some(prost_types::value::Kind::StringValue(
            "live-test".to_owned()
        ))
    );
}

#[test]
fn namespaces_duplicate_mcp_tool_names() {
    let mut used = HashSet::from(["duplicate".to_owned()]);

    let name = unique_tool_name("duplicate", "Second server", &mut used);

    assert_eq!(name, "mcp__Second_server__duplicate");
}
