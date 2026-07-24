use std::{collections::BTreeMap, path::PathBuf};

use flit_providers::{
    CodexContractError, CodexManagedScope, CodexManagedThreadId, CodexManagedTurnId,
    CodexThreadState, CodexTurnObservation, CodexTurnTerminalOutcome,
    MAX_CODEX_APP_SERVER_FRAME_BYTES, MAX_CODEX_MANAGED_THREADS, MAX_CODEX_TURN_PROMPT_BYTES,
    codex_initialize_request, codex_initialized_notification, codex_read_only_start_request,
    codex_read_request, codex_thread_list_request, codex_turn_interrupt_request,
    codex_turn_start_request, decode_codex_read_response, decode_codex_start_response,
    decode_codex_thread_list_response, decode_codex_turn_interrupt_response,
    decode_codex_turn_notification, decode_codex_turn_start_response,
};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("../fixtures/codex-0.144.6-contract.jsonl");
const CWD: &str = "/private/tmp/flit-synthetic-repo";

#[test]
fn requests_preserve_validated_methods_and_read_only_policy() {
    let initialize: Value =
        serde_json::from_slice(&codex_initialize_request(1).expect("initialize request"))
            .expect("initialize JSON");
    assert_eq!(initialize["id"], 1);
    assert_eq!(initialize["method"], "initialize");
    assert_eq!(
        initialize["params"]["capabilities"]["experimentalApi"],
        true
    );
    assert_eq!(initialize["params"]["clientInfo"]["name"], "flit");

    let initialized: Value = serde_json::from_slice(&codex_initialized_notification())
        .expect("initialized notification");
    assert_eq!(initialized, json!({"method": "initialized", "params": {}}));

    let start: Value =
        serde_json::from_slice(&codex_read_only_start_request(2, CWD).expect("start request"))
            .expect("start JSON");
    assert_eq!(start["method"], fixture_message("start-request")["method"]);
    assert_eq!(start["params"]["cwd"], CWD);
    assert_eq!(start["params"]["sandbox"], "read-only");
    assert_eq!(start["params"]["approvalPolicy"], "never");
    assert_eq!(start["params"]["ephemeral"], false);
    assert_eq!(start["params"]["serviceName"], "flit");
    assert_eq!(start["params"]["threadSource"], "flit");

    let list: Value =
        serde_json::from_slice(&codex_thread_list_request(3, CWD, Some("page-2")).expect("list"))
            .expect("list JSON");
    assert_eq!(
        list["method"],
        fixture_message("managed-list-request")["method"]
    );
    assert_eq!(list["params"], json!({"cwd": CWD, "cursor": "page-2"}));

    let thread_id = thread_id("managed-1");
    let read: Value =
        serde_json::from_slice(&codex_read_request(4, &thread_id).expect("read request"))
            .expect("read JSON");
    assert_eq!(
        read["method"],
        fixture_message("disconnect-recovery-read-request")["method"]
    );
    assert_eq!(
        read["params"],
        json!({"threadId": "managed-1", "includeTurns": true})
    );
}

#[test]
fn start_requires_exact_identity_request_and_effective_policy() {
    let mut response = fixture_message("start-response-selected-fields");
    replace_placeholders(&mut response, "<thread-id>", "managed-1");
    let started =
        decode_codex_start_response(&json_bytes(&response), 2, CWD).expect("valid start response");
    assert_eq!(started.thread_id, thread_id("managed-1"));
    assert_eq!(started.canonical_cwd, PathBuf::from(CWD));

    let mut mismatched_session = response.clone();
    mismatched_session["result"]["thread"]["sessionId"] = json!("other");
    assert_eq!(
        decode_codex_start_response(&json_bytes(&mismatched_session), 2, CWD),
        Err(CodexContractError::MismatchedSessionIdentity)
    );

    let mut wrong_policy = response.clone();
    wrong_policy["result"]["approvalPolicy"] = json!("on-request");
    assert_eq!(
        decode_codex_start_response(&json_bytes(&wrong_policy), 2, CWD),
        Err(CodexContractError::UnexpectedEffectivePolicy)
    );
    assert_eq!(
        decode_codex_start_response(&json_bytes(&response), 99, CWD),
        Err(CodexContractError::UnexpectedRequestId)
    );
}

#[test]
fn turn_requests_and_responses_bind_exact_managed_identity() {
    let thread_id = thread_id("managed-1");
    let start: Value = serde_json::from_slice(
        &codex_turn_start_request(4, &thread_id, "Respond with FLIT.").expect("turn start"),
    )
    .expect("turn start JSON");
    assert_eq!(
        start["method"],
        fixture_message("turn-start-request")["method"]
    );
    assert_eq!(start["params"]["threadId"], "managed-1");
    assert_eq!(
        start["params"]["input"],
        json!([{"type": "text", "text": "Respond with FLIT."}])
    );

    let start_response = json!({"id": 4, "result": {"turn": {"id": "turn-1"}}});
    let started = decode_codex_turn_start_response(&json_bytes(&start_response), 4, &thread_id)
        .expect("turn start response");
    assert_eq!(started.thread_id, thread_id);
    assert_eq!(started.turn_id, turn_id("turn-1"));

    let interrupt: Value = serde_json::from_slice(
        &codex_turn_interrupt_request(5, &started.thread_id, &started.turn_id)
            .expect("interrupt request"),
    )
    .expect("interrupt JSON");
    assert_eq!(
        interrupt["method"],
        fixture_message("interrupt-request")["method"]
    );
    assert_eq!(
        interrupt["params"],
        json!({"threadId": "managed-1", "turnId": "turn-1"})
    );
    let receipt = decode_codex_turn_interrupt_response(
        &json_bytes(&json!({"id": 5, "result": {}})),
        5,
        &started.thread_id,
        &started.turn_id,
    )
    .expect("interrupt response");
    assert_eq!(receipt.thread_id, started.thread_id);
    assert_eq!(receipt.turn_id, started.turn_id);
}

#[test]
fn selected_turn_notifications_require_exact_identity_and_variant() {
    let expected_thread = thread_id("managed-1");
    let expected_turn = turn_id("turn-1");
    let mut command = fixture_message("activity-command-started");
    replace_placeholders(&mut command, "<thread-id>", "managed-1");
    replace_placeholders(&mut command, "<turn-id>", "turn-1");
    replace_placeholders(&mut command, "<item-id>", "item-1");
    replace_placeholders(&mut command, "<synthetic-repo>", CWD);
    assert_eq!(
        decode_codex_turn_notification(&json_bytes(&command), &expected_thread, &expected_turn),
        Ok(Some(CodexTurnObservation::CommandStarted {
            thread_id: expected_thread.clone(),
            turn_id: expected_turn.clone(),
            item_id: flit_providers::CodexManagedItemId::new("item-1").expect("item ID"),
        }))
    );

    let mut completed = fixture_message("normal-completion");
    replace_placeholders(&mut completed, "<thread-id>", "managed-1");
    replace_placeholders(&mut completed, "<turn-id>", "turn-1");
    assert_eq!(
        decode_codex_turn_notification(&json_bytes(&completed), &expected_thread, &expected_turn),
        Ok(Some(CodexTurnObservation::Terminal {
            thread_id: expected_thread.clone(),
            turn_id: expected_turn.clone(),
            outcome: CodexTurnTerminalOutcome::Completed,
        }))
    );

    let mut interrupted = fixture_message("interrupt-terminal");
    replace_placeholders(&mut interrupted, "<thread-id>", "managed-1");
    replace_placeholders(&mut interrupted, "<turn-id>", "turn-1");
    assert_eq!(
        decode_codex_turn_notification(&json_bytes(&interrupted), &expected_thread, &expected_turn),
        Ok(Some(CodexTurnObservation::Terminal {
            thread_id: expected_thread.clone(),
            turn_id: expected_turn.clone(),
            outcome: CodexTurnTerminalOutcome::Interrupted,
        }))
    );

    let unselected = json!({"method": "item/completed", "params": {}});
    assert_eq!(
        decode_codex_turn_notification(&json_bytes(&unselected), &expected_thread, &expected_turn),
        Ok(None)
    );

    command["params"]["turnId"] = json!("other-turn");
    assert_eq!(
        decode_codex_turn_notification(&json_bytes(&command), &expected_thread, &expected_turn),
        Err(CodexContractError::UnexpectedTurnId)
    );
    completed["params"]["turn"]["status"] = json!("failed");
    assert_eq!(
        decode_codex_turn_notification(&json_bytes(&completed), &expected_thread, &expected_turn),
        Err(CodexContractError::UnexpectedTurnStatus)
    );
}

#[test]
fn list_uses_only_exact_ids_and_exact_canonical_cwd() {
    let scope = CodexManagedScope::new(
        CWD,
        [
            thread_id("managed-1"),
            thread_id("managed-2"),
            thread_id("missing"),
        ],
    )
    .expect("managed scope");
    let response = json!({
        "id": 3,
        "result": {
            "data": [
                {
                    "id": "managed-1",
                    "sessionId": "managed-1",
                    "cwd": CWD,
                    "source": "untrusted-provider-label"
                },
                {
                    "id": "managed-2",
                    "sessionId": "managed-2",
                    "cwd": "/private/tmp/different-project",
                    "source": "flit"
                },
                {
                    "id": "unrelated",
                    "sessionId": "unrelated",
                    "cwd": CWD,
                    "source": "flit"
                }
            ],
            "nextCursor": null
        }
    });

    let page = decode_codex_thread_list_response(&json_bytes(&response), 3, &scope)
        .expect("valid list response");
    assert_eq!(page.matched_thread_ids, [thread_id("managed-1")]);
    assert_eq!(page.conflicting_threads.len(), 1);
    assert_eq!(
        page.conflicting_threads[0].thread_id,
        thread_id("managed-2")
    );
    assert_eq!(page.unseen_exact_thread_ids, [thread_id("missing")]);
    assert_eq!(page.unrelated_thread_count, 1);
    assert_eq!(page.next_cursor, None);
}

#[test]
fn read_replays_fixture_without_inventing_unknown_completion() {
    let expected_id = thread_id("managed-1");
    let mut response = fixture_message("disconnect-recovery-read");
    replace_placeholders(&mut response, "<thread-id>", "managed-1");
    replace_placeholders(&mut response, "<completed-turn-id>", "turn-1");
    replace_placeholders(&mut response, "<disconnected-turn-id>", "turn-2");
    let read = decode_codex_read_response(&json_bytes(&response), 3, &expected_id)
        .expect("valid read response");
    assert_eq!(read.thread_id, expected_id);
    assert_eq!(read.latest_turn_id.as_deref(), Some("turn-2"));
    assert_eq!(read.state, CodexThreadState::Interrupted);

    let unknown = json!({
        "id": 3,
        "result": {
            "thread": {
                "id": "managed-1",
                "sessionId": "managed-1",
                "turns": [{"id": "turn-3", "status": "newProviderStatus"}]
            }
        }
    });
    assert_eq!(
        decode_codex_read_response(&json_bytes(&unknown), 3, &thread_id("managed-1"))
            .expect("unknown read")
            .state,
        CodexThreadState::Unknown
    );

    let missing_turn_identity = json!({
        "id": 3,
        "result": {
            "thread": {
                "id": "managed-1",
                "sessionId": "managed-1",
                "turns": [{"status": "completed"}]
            }
        }
    });
    assert_eq!(
        decode_codex_read_response(
            &json_bytes(&missing_turn_identity),
            3,
            &thread_id("managed-1")
        )
        .expect("identity-free completion remains unknown")
        .state,
        CodexThreadState::Unknown
    );

    for non_terminal_status in ["inProgress", "pending"] {
        let non_terminal = json!({
            "id": 3,
            "result": {
                "thread": {
                    "id": "managed-1",
                    "sessionId": "managed-1",
                    "turns": [{"id": "turn-live", "status": non_terminal_status}]
                }
            }
        });
        assert_eq!(
            decode_codex_read_response(&json_bytes(&non_terminal), 3, &thread_id("managed-1"))
                .expect("persisted non-terminal state remains unknown")
                .state,
            CodexThreadState::Unknown
        );
    }

    for terminal_status in ["completed", "failed"] {
        let terminal = json!({
            "id": 3,
            "result": {
                "thread": {
                    "id": "managed-1",
                    "sessionId": "managed-1",
                    "turns": [{"id": "turn-terminal", "status": terminal_status}]
                }
            }
        });
        let expected_state = if terminal_status == "completed" {
            CodexThreadState::Completed
        } else {
            CodexThreadState::Failed
        };
        assert_eq!(
            decode_codex_read_response(&json_bytes(&terminal), 3, &thread_id("managed-1"))
                .expect("terminal read")
                .state,
            expected_state
        );
    }
}

#[test]
fn malformed_oversized_duplicate_and_over_count_responses_fail_closed() {
    assert_eq!(
        decode_codex_start_response(b"{", 2, CWD),
        Err(CodexContractError::MalformedJson)
    );
    assert_eq!(
        decode_codex_start_response(&vec![b' '; MAX_CODEX_APP_SERVER_FRAME_BYTES + 1], 2, CWD),
        Err(CodexContractError::FrameTooLarge)
    );

    let scope = CodexManagedScope::new(CWD, [thread_id("managed-1")]).expect("managed exact scope");
    let duplicate = json!({
        "id": 3,
        "result": {
            "data": [
                {"id": "managed-1", "sessionId": "managed-1", "cwd": CWD},
                {"id": "managed-1", "sessionId": "managed-1", "cwd": CWD}
            ],
            "nextCursor": null
        }
    });
    assert_eq!(
        decode_codex_thread_list_response(&json_bytes(&duplicate), 3, &scope),
        Err(CodexContractError::DuplicateThreadId)
    );

    let threads = (0..=MAX_CODEX_MANAGED_THREADS)
        .map(|index| {
            json!({
                "id": format!("thread-{index}"),
                "sessionId": format!("thread-{index}"),
                "cwd": CWD
            })
        })
        .collect::<Vec<_>>();
    let over_count = json!({
        "id": 3,
        "result": {"data": threads, "nextCursor": null}
    });
    assert_eq!(
        decode_codex_thread_list_response(&json_bytes(&over_count), 3, &scope),
        Err(CodexContractError::TooManyThreads)
    );
}

#[test]
fn invalid_caller_scope_is_rejected_before_any_provider_frame() {
    assert_eq!(
        codex_read_only_start_request(1, "relative/project"),
        Err(CodexContractError::InvalidCanonicalCwd)
    );
    assert_eq!(
        codex_thread_list_request(1, "/private/tmp/../tmp/project", None),
        Err(CodexContractError::InvalidCanonicalCwd)
    );
    assert_eq!(
        codex_initialize_request(9_007_199_254_740_992),
        Err(CodexContractError::InvalidRequestId)
    );
    assert_eq!(
        CodexManagedScope::new(CWD, []),
        Err(CodexContractError::InvalidManagedScope)
    );
    assert_eq!(
        CodexManagedScope::new(CWD, [thread_id("duplicate"), thread_id("duplicate")]),
        Err(CodexContractError::InvalidManagedScope)
    );
    assert_eq!(
        CodexManagedThreadId::new(" \t"),
        Err(CodexContractError::InvalidThreadId)
    );
    assert_eq!(
        CodexManagedTurnId::new("\n"),
        Err(CodexContractError::InvalidTurnId)
    );
    assert_eq!(
        codex_turn_start_request(1, &thread_id("managed-1"), ""),
        Err(CodexContractError::InvalidTurnPrompt)
    );
    assert_eq!(
        codex_turn_start_request(
            1,
            &thread_id("managed-1"),
            &"x".repeat(MAX_CODEX_TURN_PROMPT_BYTES + 1)
        ),
        Err(CodexContractError::InvalidTurnPrompt)
    );

    let oversized_cwd = format!("/{}", "x".repeat(MAX_CODEX_APP_SERVER_FRAME_BYTES));
    assert_eq!(
        codex_read_only_start_request(1, &oversized_cwd),
        Err(CodexContractError::FrameTooLarge)
    );
    assert_eq!(
        codex_thread_list_request(1, &oversized_cwd, None),
        Err(CodexContractError::FrameTooLarge)
    );
}

fn fixture_message(name: &str) -> Value {
    fixture_records()
        .remove(name)
        .unwrap_or_else(|| panic!("missing fixture {name}"))["message"]
        .clone()
}

fn fixture_records() -> BTreeMap<String, Value> {
    FIXTURE
        .lines()
        .map(|line| {
            let value: Value = serde_json::from_str(line).expect("fixture JSON");
            let name = value["fixture"].as_str().expect("fixture name").to_owned();
            (name, value)
        })
        .collect()
}

fn replace_placeholders(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::Array(values) => {
            for value in values {
                replace_placeholders(value, from, to);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                replace_placeholders(value, from, to);
            }
        }
        Value::String(string) if string == from => *string = to.to_owned(),
        _ => {}
    }
}

fn thread_id(value: &str) -> CodexManagedThreadId {
    CodexManagedThreadId::new(value).expect("valid test thread ID")
}

fn turn_id(value: &str) -> CodexManagedTurnId {
    CodexManagedTurnId::new(value).expect("valid test turn ID")
}

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("test JSON")
}
