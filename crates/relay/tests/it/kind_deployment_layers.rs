//! Offline check that the three layers `relay` renders for a Kubernetes
//! deployment agree with each other: the CRD (`relay k8s crd render`), the
//! operator control plane (`relay k8s operator render`) and the instance
//! custom resource (`relay k8s instance render`). Runs against the COMPILED
//! `relay` binary in the default build. No cluster, no network, no kubectl.

use std::process::Command;

use serde::Deserialize;

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
fn parse_yaml_docs(what: &str, yaml: &str) -> Vec<serde_yaml::Value> {
    let mut docs = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(yaml) {
        let value = serde_yaml::Value::deserialize(doc)
            .unwrap_or_else(|e| panic!("{what}: YAML document does not parse: {e}"));
        if !value.is_null() {
            assert!(value.is_mapping(), "{what}: document is not a mapping");
            docs.push(value);
        }
    }
    assert!(!docs.is_empty(), "{what}: no YAML documents emitted");
    docs
}

fn first_doc(args: &[&str]) -> serde_yaml::Value {
    let what = args.join(" ");
    parse_yaml_docs(&what, &stdout(args))
        .into_iter()
        .next()
        .expect("first document")
}

fn operator_docs(args: &[&str]) -> Vec<serde_yaml::Value> {
    let what = args.join(" ");
    parse_yaml_docs(&what, &stdout(args))
}

/// The position of the first document of `kind` in an operator stream.
fn position_of(docs: &[serde_yaml::Value], kind: &str) -> Result<usize, String> {
    docs.iter()
        .position(|d| d["kind"].as_str() == Some(kind))
        .ok_or_else(|| format!("operator layer has no {kind} document"))
}

fn text<'a>(v: &'a serde_yaml::Value, what: &str) -> Result<&'a str, String> {
    v.as_str()
        .ok_or_else(|| format!("{what} is missing or not a string"))
}

/// True when the YAML sequence `seq` holds the string `wanted`.
fn holds(seq: &serde_yaml::Value, wanted: &str) -> bool {
    seq.as_sequence()
        .is_some_and(|items| items.iter().any(|x| x.as_str() == Some(wanted)))
}

/// Check that the three layers fit together. Returns `Err` with a message
/// that names the first broken rule, `Ok(())` when every rule holds.
fn layers_agree(
    crd: &serde_yaml::Value,
    operator: &[serde_yaml::Value],
    instance: &serde_yaml::Value,
    namespace: &str,
) -> Result<(), String> {
    // apiVersion: the instance uses the CRD group and its served + storage version.
    let group = text(&crd["spec"]["group"], "CRD group")?;
    let version = &crd["spec"]["versions"][0];
    let version_name = text(&version["name"], "CRD version name")?;
    let expected_api = format!("{group}/{version_name}");
    let got_api = text(&instance["apiVersion"], "instance apiVersion")?;
    if got_api != expected_api {
        return Err(format!(
            "apiVersion: instance has {got_api}, CRD serves {expected_api}"
        ));
    }
    if version["served"].as_bool() != Some(true) || version["storage"].as_bool() != Some(true) {
        return Err(format!(
            "apiVersion: CRD version {version_name} must be served and storage"
        ));
    }

    // kind: the instance kind is the CRD kind.
    let crd_kind = text(&crd["spec"]["names"]["kind"], "CRD kind")?;
    let got_kind = text(&instance["kind"], "instance kind")?;
    if got_kind != crd_kind {
        return Err(format!(
            "kind: instance has {got_kind}, CRD names {crd_kind}"
        ));
    }

    // plural: the operator may manage the CRD plural and its status.
    let plural = text(&crd["spec"]["names"]["plural"], "CRD plural")?;
    let status = format!("{plural}/status");
    let role = &operator[position_of(operator, "ClusterRole")?];
    let plural_granted = role["rules"].as_sequence().is_some_and(|rules| {
        rules.iter().any(|rule| {
            holds(&rule["apiGroups"], group)
                && holds(&rule["resources"], plural)
                && holds(&rule["resources"], &status)
        })
    });
    if !plural_granted {
        return Err(format!(
            "plural: ClusterRole grants no rule for {group} {plural} and {status}"
        ));
    }

    // spec field: every instance spec key is a CRD schema property.
    let allowed: Vec<&str> = crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]
        ["spec"]["properties"]
        .as_mapping()
        .ok_or("spec field: CRD schema has no spec properties")?
        .keys()
        .filter_map(|k| k.as_str())
        .collect();
    let spec = instance["spec"]
        .as_mapping()
        .ok_or("spec field: instance has no spec mapping")?;
    for key in spec.keys() {
        let name = key.as_str().unwrap_or("");
        if !allowed.contains(&name) {
            return Err(format!("spec field {name} is not in the CRD schema"));
        }
    }

    // binding: the ClusterRoleBinding points at the ClusterRole and the ServiceAccount.
    let binding = &operator[position_of(operator, "ClusterRoleBinding")?];
    let sa = &operator[position_of(operator, "ServiceAccount")?];
    let sa_name = text(&sa["metadata"]["name"], "ServiceAccount name")?;
    let sa_namespace = text(&sa["metadata"]["namespace"], "ServiceAccount namespace")?;
    let role_name = text(&role["metadata"]["name"], "ClusterRole name")?;
    if text(&binding["roleRef"]["name"], "binding roleRef name")? != role_name {
        return Err(format!(
            "binding: roleRef does not name ClusterRole {role_name}"
        ));
    }
    let subject = &binding["subjects"][0];
    if subject["kind"].as_str() != Some("ServiceAccount")
        || subject["name"].as_str() != Some(sa_name)
        || subject["namespace"].as_str() != Some(sa_namespace)
    {
        return Err(format!(
            "binding: first subject is not ServiceAccount {sa_name} in {sa_namespace}"
        ));
    }

    // service account: the operator Deployment runs as that ServiceAccount.
    let deployment = &operator[position_of(operator, "Deployment")?];
    let deploy_sa = text(
        &deployment["spec"]["template"]["spec"]["serviceAccountName"],
        "Deployment serviceAccountName",
    )?;
    if deploy_sa != sa_name {
        return Err(format!(
            "service account: Deployment runs as {deploy_sa}, ServiceAccount is {sa_name}"
        ));
    }

    // namespace: every namespaced object lands in the given namespace.
    let ns_doc = &operator[position_of(operator, "Namespace")?];
    let checks = [
        (
            "Namespace",
            text(&ns_doc["metadata"]["name"], "Namespace name")?,
        ),
        ("ServiceAccount", sa_namespace),
        (
            "Deployment",
            text(&deployment["metadata"]["namespace"], "Deployment namespace")?,
        ),
        (
            "binding subject",
            text(&subject["namespace"], "binding subject namespace")?,
        ),
    ];
    for (what, got) in checks {
        if got != namespace {
            return Err(format!(
                "namespace: {what} is in {got}, expected {namespace}"
            ));
        }
    }

    Ok(())
}

/// The good CRD, operator (namespace `kind-ops`) and prod instance layers.
fn good_layers() -> (serde_yaml::Value, Vec<serde_yaml::Value>, serde_yaml::Value) {
    let crd = first_doc(&["k8s", "crd", "render"]);
    let operator = operator_docs(&["k8s", "operator", "render", "--namespace", "kind-ops"]);
    let instance = first_doc(&["k8s", "instance", "render", "--profile", "prod"]);
    (crd, operator, instance)
}

#[test]
fn rendered_layers_agree_for_every_profile() {
    let crd = first_doc(&["k8s", "crd", "render"]);
    let operator = operator_docs(&["k8s", "operator", "render", "--namespace", "kind-ops"]);
    for profile in ["dev", "staging", "prod", "template"] {
        let instance = first_doc(&["k8s", "instance", "render", "--profile", profile]);
        assert_eq!(
            layers_agree(&crd, &operator, &instance, "kind-ops"),
            Ok(()),
            "profile {profile}: the CRD, operator and instance layers must agree"
        );
    }
}

#[test]
fn operator_namespace_flag_reaches_every_namespaced_object() {
    let raw = stdout(&["k8s", "operator", "render", "--namespace", "kind-ops"]);
    assert!(
        !raw.contains("relay-system"),
        "operator render with --namespace kind-ops must not mention relay-system"
    );
    let docs = operator_docs(&["k8s", "operator", "render", "--namespace", "kind-ops"]);
    let ns = &docs[position_of(&docs, "Namespace").expect("Namespace")];
    assert_eq!(
        ns["metadata"]["name"].as_str(),
        Some("kind-ops"),
        "Namespace must be kind-ops"
    );
    for kind in ["ServiceAccount", "Deployment"] {
        let doc = &docs[position_of(&docs, kind).expect(kind)];
        assert_eq!(
            doc["metadata"]["namespace"].as_str(),
            Some("kind-ops"),
            "{kind} must be in kind-ops"
        );
    }

    let default_docs = operator_docs(&["k8s", "operator", "render"]);
    let default_ns = &default_docs[position_of(&default_docs, "Namespace").expect("Namespace")];
    assert_eq!(
        default_ns["metadata"]["name"].as_str(),
        Some("relay-system"),
        "without --namespace the Namespace is relay-system"
    );
    let crd = first_doc(&["k8s", "crd", "render"]);
    let instance = first_doc(&["k8s", "instance", "render", "--profile", "prod"]);
    assert_eq!(
        layers_agree(&crd, &default_docs, &instance, "relay-system"),
        Ok(()),
        "default operator namespace must agree with the CRD and instance layers"
    );
}

#[test]
fn instance_overrides_stay_inside_the_crd_schema() {
    let crd = first_doc(&["k8s", "crd", "render"]);
    let operator = operator_docs(&["k8s", "operator", "render", "--namespace", "kind-ops"]);
    let instance = first_doc(&[
        "k8s",
        "instance",
        "render",
        "--profile",
        "dev",
        "--name",
        "relay",
        "--namespace",
        "queues",
        "--image",
        "relay:pinned",
    ]);
    assert_eq!(
        layers_agree(&crd, &operator, &instance, "kind-ops"),
        Ok(()),
        "instance with --name/--namespace/--image must agree with the CRD and operator"
    );
    assert_eq!(
        instance["spec"]["image"].as_str(),
        Some("relay:pinned"),
        "--image must reach spec.image"
    );
}

#[test]
fn layers_agree_rejects_a_drifted_layer() {
    let (crd, operator, instance) = good_layers();
    assert_eq!(
        layers_agree(&crd, &operator, &instance, "kind-ops"),
        Ok(()),
        "the unmodified layers must agree before any drift is injected"
    );

    // 1. apiVersion drift.
    let mut drifted = instance.clone();
    drifted["apiVersion"] = serde_yaml::Value::from("relay.dev/v1beta1");
    let err = layers_agree(&crd, &operator, &drifted, "kind-ops")
        .expect_err("apiVersion drift must be rejected");
    assert!(
        err.contains("apiVersion"),
        "message names apiVersion: {err}"
    );

    // 2. kind drift.
    let mut drifted = instance.clone();
    drifted["kind"] = serde_yaml::Value::from("Relays");
    let err = layers_agree(&crd, &operator, &drifted, "kind-ops")
        .expect_err("kind drift must be rejected");
    assert!(err.contains("kind"), "message names kind: {err}");

    // 3. spec field not in the CRD schema.
    let mut drifted = instance.clone();
    drifted["spec"]["notAField"] = serde_yaml::Value::from(1);
    let err = layers_agree(&crd, &operator, &drifted, "kind-ops")
        .expect_err("unknown spec field must be rejected");
    assert!(
        err.contains("spec field"),
        "message names spec field: {err}"
    );

    // 4. plural drift in the ClusterRole.
    let mut drifted_ops = operator.clone();
    let role = position_of(&drifted_ops, "ClusterRole").expect("ClusterRole");
    drifted_ops[role]["rules"][0]["resources"][0] = serde_yaml::Value::from("widgets");
    let err = layers_agree(&crd, &drifted_ops, &instance, "kind-ops")
        .expect_err("plural drift must be rejected");
    assert!(err.contains("plural"), "message names plural: {err}");

    // 5. binding subject drift.
    let mut drifted_ops = operator.clone();
    let binding = position_of(&drifted_ops, "ClusterRoleBinding").expect("binding");
    drifted_ops[binding]["subjects"][0]["name"] = serde_yaml::Value::from("other");
    let err = layers_agree(&crd, &drifted_ops, &instance, "kind-ops")
        .expect_err("binding drift must be rejected");
    assert!(err.contains("binding"), "message names binding: {err}");

    // 6. Deployment service account drift.
    let mut drifted_ops = operator.clone();
    let deploy = position_of(&drifted_ops, "Deployment").expect("Deployment");
    drifted_ops[deploy]["spec"]["template"]["spec"]["serviceAccountName"] =
        serde_yaml::Value::from("other");
    let err = layers_agree(&crd, &drifted_ops, &instance, "kind-ops")
        .expect_err("service account drift must be rejected");
    assert!(
        err.contains("service account"),
        "message names service account: {err}"
    );

    // 7. namespace drift: the good layers checked against another namespace.
    let err = layers_agree(&crd, &operator, &instance, "elsewhere")
        .expect_err("namespace drift must be rejected");
    assert!(err.contains("namespace"), "message names namespace: {err}");
}
