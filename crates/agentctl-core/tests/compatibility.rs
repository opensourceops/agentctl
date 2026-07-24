use agentctl_core::compiler::TaskUse;
use agentctl_core::{compile, parse_workflow};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Expected {
    migrated_legacy: bool,
    workflow_name: String,
    order: Vec<String>,
    task: ExpectedTask,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedTask {
    id: String,
    use_kind: String,
    reference: String,
    needs: Vec<String>,
}

#[test]
fn typescript_assign_fixture_translates_to_the_language_neutral_contract() {
    let source = include_str!("../../../fixtures/compat/v0/assign.playbook.yaml");
    let expected: Expected = serde_json::from_str(include_str!(
        "../../../fixtures/compat/v0/assign.expected.json"
    ))
    .expect("expected fixture");
    let parsed = parse_workflow(source, "assign.playbook.yaml").expect("legacy parse");
    let plan = compile(&parsed.workflow, "assign.playbook.yaml").expect("legacy compile");
    assert_eq!(parsed.migrated_legacy, expected.migrated_legacy);
    assert_eq!(plan.workflow_name, expected.workflow_name);
    assert_eq!(plan.order, expected.order);
    let task = plan.tasks.get(&expected.task.id).expect("task");
    assert_eq!(task.needs, expected.task.needs);
    match &task.uses {
        TaskUse::Action(reference) => {
            assert_eq!(expected.task.use_kind, "action");
            assert_eq!(reference, &expected.task.reference);
        }
        TaskUse::Agent(_)
        | TaskUse::Aggregate(_)
        | TaskUse::Router(_)
        | TaskUse::LoopAggregate(_) => {
            panic!("expected action task")
        }
    }
}
