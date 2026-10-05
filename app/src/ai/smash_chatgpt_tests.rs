use super::*;

#[test]
fn parses_text_and_function_calls_from_responses_sse() {
    let stream = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"run_shell_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
        "data: [DONE]\n\n",
    );
    let output = parse_response_stream(stream).unwrap();
    assert_eq!(output[0]["text"], "hello");
    assert_eq!(output[1]["id"], "call_1");
    assert_eq!(output[1]["input"]["command"], "pwd");
}

#[test]
fn parses_picker_visible_models_in_server_priority_order() {
    let catalog = r#"{
        "models": [
            {
                "slug": "gpt-5.6-sol",
                "display_name": "GPT-5.6 Sol",
                "description": "Everyday work",
                "priority": 20,
                "visibility": "list"
            },
            {
                "slug": "gpt-6-astra",
                "display_name": "GPT-6 Astra",
                "description": "Complex work",
                "priority": 10,
                "visibility": "list"
            },
            {
                "slug": "gpt-internal",
                "display_name": "Internal",
                "priority": 0,
                "visibility": "hide"
            }
        ]
    }"#;

    let models = parse_model_catalog(catalog).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-6-astra");
    assert_eq!(models[0].display_name, "GPT-6 Astra");
    assert_eq!(models[1].id, "gpt-5.6-sol");
}

#[test]
fn discovers_new_model_ids_without_a_fixed_list() {
    let models = parse_model_catalog(r#"{"models":[{"slug":"gpt-future"}]}"#).unwrap();

    assert_eq!(models[0].id, "gpt-future");
    assert_eq!(models[0].display_name, "gpt-future");
}

#[test]
fn rejects_catalogs_without_available_models() {
    assert!(parse_model_catalog(r#"{"models":[]}"#).is_err());
    assert!(
        parse_model_catalog(r#"{"models":[{"slug":"gpt-internal","visibility":"hide"}]}"#).is_err()
    );
}
