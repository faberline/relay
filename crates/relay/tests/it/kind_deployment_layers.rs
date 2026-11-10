//! Offline check that the three layers `relay` renders for a Kubernetes
//! deployment fit together: the CRD (`relay k8s crd render`), the operator
//! control plane (`relay k8s operator render`) and the instance custom
//! resource (`relay k8s instance render`). Runs against the COMPILED `relay`
//! binary in the default build. No cluster, no Kind, no network.

use std::process::Command;

use serde::Deserialize;
use serde_yaml::Value;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(args)
        .output()
        .expect("run relay binary")
}

fn stdout(args: &[&str]) -> String {
    let out = run(args);
    assert!(
        out.status.success(),
        "`relay {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

/// Parse every YAML document in a (possibly multi-doc) stream, asserting the
/// stream is non-empty and each document is a mapping.
fn parse_yaml_docs(what: &str, yaml: &str) -> Vec<Value> {
    let mut docs = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(yaml) {
        let value = Value::deserialize(doc)
            .unwrap_or_else(|e| panic!("{what}: YAML document does not parse: {e}"));
        if !value.is_null() {
            assert!(value.is_mapping(), "{what}: document is not a mapping");
            docs.push(value);
        }
    }
    assert!(!docs.is_empty(), "{what}: no YAML documents emitted");
    docs
}

fn crd_layer() -> Value {
    parse_yaml_docs("crd render", &stdout(&["k8s", "crd", "render"]))
        .into_iter()
        .next()
        .expect("one CRD document")
}

fn operator_layer(namespace: &str) -> Vec<Value> {
    parse_yaml_docs(
        "operator render",
        &stdout(&["k8s", "operator", "render", "--namespace", namespace]),
    )
}

fn instance_layer(args: &[&str]) -> Value {
    let mut full = vec!["k8s", "instance", "render"];
    full.extend_from_slice(args);
    parse_yaml_docs(&full.join(" "), &stdout(&full))
        .into_iter()
        .next()
        .expect("one instance document")
}

fn str_at<'a>(v: &'a Value, what: &str) -> Result<&'a str, String> {
    v.as_str()
        .ok_or_else(|| format!("{what} is missing or not a string"))
}

fn find_kind<'a>(docs: &'a [Value], kind: &str) -> Result<&'a Value, String> {
    docs.iter()
        .find(|d| d["kind"].as_str() == Some(kind))
        .ok_or_else(|| format!("{kind} document missing from operator render"))
}

fn kind_index(docs: &[Value], kind: &str) -> usize {
    docs.iter()
        .position(|d| d["kind"].as_str() == Some(kind))
        .unwrap_or_else(|| panic!("{kind} document missing from operator render"))
}

/// Check that the three layers agree. Returns `Err` with a message that
/// contains the quoted rule word for the first broken rule.
fn layers_agree(
    crd: &Value,
    operator: &[Value],
    instance: &Value,
    namespace: &str,
) -> Result<(), String> {
    // apiVersion: the instance's apiVersion is the CRD group and served version.
    let group = str_at(&crd["spec"]["group"], "CRD spec.group")?;
    let version = &crd["spec"]["versions"][0];
    let version_name = str_at(&version["name"], "CRD version name")?;
    if version["served"].as_bool() != Some(true) || version["storage"].as_bool() != Some(true) {
        return Err("apiVersion: CRD version must be served and storage".into());
    }
    let api_version = format!("{group}/{version_name}");
    let instance_api = str_at(&instance["apiVersion"], "instance apiVersion")?;
    if instance_api != api_version {
        return Err(format!(
            "apiVersion: instance {instance_api} != CRD {api_version}"
        ));
    }

    // kind: the instance kind is the CRD kind.
    let crd_kind = str_at(&crd["spec"]["names"]["kind"], "CRD kind")?;
    let instance_kind = str_at(&instance["kind"], "instance kind")?;
    if instance_kind != crd_kind {
        return Err(format!("kind: instance {instance_kind} != CRD {crd_kind}"));
    }

    // plural: the operator ClusterRole grants the CRD plural and its status.
    let plural = str_at(&crd["spec"]["names"]["plural"], "CRD plural")?;
    let status = format!("{plural}/status");
    let cluster_role = find_kind(operator, "ClusterRole")?;
    let granted = cluster_role["rules"]
        .as_sequence()
        .into_iter()
        .flatten()
        .any(|rule| {
            let groups_ok = rule["apiGroups"]
                .as_sequence()
                .is_some_and(|g| g.iter().any(|x| x.as_str() == Some(group)));
            let resources_ok = rule["resources"].as_sequence().is_some_and(|r| {
                r.iter().any(|x| x.as_str() == Some(plural))
                    && r.iter().any(|x| x.as_str() == Some(status.as_str()))
            });
            groups_ok && resources_ok
        });
    if !granted {
        return Err(format!(
            "plural: ClusterRole grants no rule for {group} {plural} and {status}"
        ));
    }

    // spec field: every instance spec key is a CRD schema property.
    let schema_props = crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]
        ["spec"]["properties"]
        .as_mapping()
        .ok_or("spec field: CRD schema has no spec properties")?;
    if let Some(spec) = instance["spec"].as_mapping() {
        for key in spec.keys() {
            if !schema_props.keys().any(|k| k == key) {
                return Err(format!("spec field: {key:?} is not a CRD schema property"));
            }
        }
    }

    // binding: the ClusterRoleBinding points at the ClusterRole and the ServiceAccount.
    let role_name = str_at(&cluster_role["metadata"]["name"], "ClusterRole name")?;
    let binding = find_kind(operator, "ClusterRoleBinding")?;
    let role_ref = str_at(&binding["roleRef"]["name"], "binding roleRef name")?;
    if role_ref != role_name {
        return Err(format!(
            "binding: roleRef {role_ref} != ClusterRole {role_name}"
        ));
    }
    let service_account = find_kind(operator, "ServiceAccount")?;
    let sa_name = str_at(&service_account["metadata"]["name"], "ServiceAccount name")?;
    let sa_ns = str_at(
        &service_account["metadata"]["namespace"],
        "ServiceAccount namespace",
    )?;
    let subject = &binding["subjects"][0];
    if subject["kind"].as_str() != Some("ServiceAccount") {
        return Err("binding: first subject is not a ServiceAccount".into());
    }
    let subject_name = str_at(&subject["name"], "binding subject name")?;
    if subject_name != sa_name {
        return Err(format!(
            "binding: subject {subject_name} != ServiceAccount {sa_name}"
        ));
    }
    let subject_ns = str_at(&subject["namespace"], "binding subject namespace")?;
    if subject_ns != sa_ns {
        return Err(format!(
            "binding: subject namespace {subject_ns} != ServiceAccount namespace {sa_ns}"
        ));
    }

    // service account: the operator Deployment runs as the ServiceAccount.
    let deployment = find_kind(operator, "Deployment")?;
    let deploy_sa = str_at(
        &deployment["spec"]["template"]["spec"]["serviceAccountName"],
        "Deployment serviceAccountName",
    )?;
    if deploy_sa != sa_name {
        return Err(format!(
            "service account: Deployment uses {deploy_sa}, ServiceAccount is {sa_name}"
        ));
    }

    // namespace: every namespaced object sits in the requested namespace.
    let ns_doc = find_kind(operator, "Namespace")?;
    let checks = [
        (
            "Namespace",
            str_at(&ns_doc["metadata"]["name"], "Namespace name")?,
        ),
        ("ServiceAccount", sa_ns),
        (
            "Deployment",
            str_at(&deployment["metadata"]["namespace"], "Deployment namespace")?,
        ),
        ("binding subject", subject_ns),
    ];
    for (what, ns) in checks {
        if ns != namespace {
            return Err(format!(
                "namespace: {what} is in {ns}, expected {namespace}"
            ));
        }
    }

    Ok(())
}

/// The layers of one consistent render set: the CRD, the operator control
/// plane in `kind-ops`, and the prod instance.
fn good_layers() -> (Value, Vec<Value>, Value) {
    (
        crd_layer(),
        operator_layer("kind-ops"),
        instance_layer(&["--profile", "prod"]),
    )
}

/// AC1: the CRD, operator and instance renders of all four profiles agree on
/// group, version, kind, plural, RBAC and service account.
#[test]
fn rendered_layers_agree_for_every_profile() {
    let crd = crd_layer();
    let operator = operator_layer("kind-ops");
    for profile in ["dev", "staging", "prod", "template"] {
        let instance = instance_layer(&["--profile", profile]);
        assert_eq!(
            layers_agree(&crd, &operator, &instance, "kind-ops"),
            Ok(()),
            "profile {profile} layers disagree"
        );
    }
}

/// AC2: `--namespace` reaches every namespaced operator object, and the
/// default `relay-system` is gone once a namespace is given.
#[test]
fn operator_namespace_flag_reaches_every_namespaced_object() {
    let operator = operator_layer("kind-ops");
    for doc in &operator {
        let text = serde_yaml::to_string(doc).expect("serialize operator document");
        assert!(
            !text.contains("relay-system"),
            "operator document still names relay-system:\n{text}"
        );
    }
    let ns = find_kind(&operator, "Namespace").unwrap();
    assert_eq!(ns["metadata"]["name"], "kind-ops");
    let sa = find_kind(&operator, "ServiceAccount").unwrap();
    assert_eq!(sa["metadata"]["namespace"], "kind-ops");
    let deploy = find_kind(&operator, "Deployment").unwrap();
    assert_eq!(deploy["metadata"]["namespace"], "kind-ops");

    // Without --namespace the default namespace is used and the layers agree on it.
    let default_ops = parse_yaml_docs("operator render", &stdout(&["k8s", "operator", "render"]));
    let default_ns = find_kind(&default_ops, "Namespace").unwrap();
    assert_eq!(default_ns["metadata"]["name"], "relay-system");
    let dev = instance_layer(&["--profile", "dev"]);
    assert_eq!(
        layers_agree(&crd_layer(), &default_ops, &dev, "relay-system"),
        Ok(())
    );
}

/// AC3: a `--name/--namespace/--image` instance still uses only CRD schema fields.
#[test]
fn instance_overrides_stay_inside_the_crd_schema() {
    let crd = crd_layer();
    let operator = operator_layer("kind-ops");
    let custom = instance_layer(&[
        "--profile",
        "dev",
        "--name",
        "relay",
        "--namespace",
        "queues",
        "--image",
        "relay:pinned",
    ]);
    assert_eq!(layers_agree(&crd, &operator, &custom, "kind-ops"), Ok(()));
    assert_eq!(custom["spec"]["image"], "relay:pinned");
}

/// AC4 (negative test): each of seven drifts between the layers is caught
/// with its own rule message.
#[test]
fn layers_agree_rejects_a_drifted_layer() {
    let (crd, operator, instance) = good_layers();
    assert_eq!(layers_agree(&crd, &operator, &instance, "kind-ops"), Ok(()));

    // 1. apiVersion drifts from the CRD version.
    let mut inst = instance.clone();
    inst["apiVersion"] = Value::from("relay.dev/v1beta1");
    expect_err(&crd, &operator, &inst, "kind-ops", "apiVersion");

    // 2. kind drifts from the CRD kind.
    let mut inst = instance.clone();
    inst["kind"] = Value::from("Relays");
    expect_err(&crd, &operator, &inst, "kind-ops", "kind");

    // 3. the instance sets a field the CRD schema does not have.
    let mut inst = instance.clone();
    inst["spec"]["notAField"] = Value::from(1);
    expect_err(&crd, &operator, &inst, "kind-ops", "spec field");

    // 4. the ClusterRole no longer grants the CRD plural.
    let mut ops = operator.clone();
    let cr = kind_index(&ops, "ClusterRole");
    ops[cr]["rules"][0]["resources"][0] = Value::from("widgets");
    expect_err(&crd, &ops, &instance, "kind-ops", "plural");

    // 5. the ClusterRoleBinding subject names another ServiceAccount.
    let mut ops = operator.clone();
    let crb = kind_index(&ops, "ClusterRoleBinding");
    ops[crb]["subjects"][0]["name"] = Value::from("other");
    expect_err(&crd, &ops, &instance, "kind-ops", "binding");

    // 6. the Deployment runs as another ServiceAccount.
    let mut ops = operator.clone();
    let dep = kind_index(&ops, "Deployment");
    ops[dep]["spec"]["template"]["spec"]["serviceAccountName"] = Value::from("other");
    expect_err(&crd, &ops, &instance, "kind-ops", "service account");

    // 7. the layers are checked against a namespace they were not rendered for.
    expect_err(&crd, &operator, &instance, "elsewhere", "namespace");
}

fn expect_err(crd: &Value, operator: &[Value], instance: &Value, ns: &str, word: &str) {
    match layers_agree(crd, operator, instance, ns) {
        Ok(()) => panic!("drift containing `{word}` was accepted"),
        Err(msg) => assert!(
            msg.contains(word),
            "expected an error naming `{word}`, got: {msg}"
        ),
    }
}
