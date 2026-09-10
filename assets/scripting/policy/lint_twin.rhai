// Twin-wide namespace policy. Rust supplies resolver facts; this policy owns
// severity and the actionable message shown by RunLint and ValidateTwin.
fn finding(rule, severity, subject, message) {
  let out = #{};
  out.rule = rule;
  out.severity = severity;
  out.subject = subject;
  out.message = message;
  out
}

fn entry_line(entry) {
  entry.owner + " [" + entry.source + "; domain=" + entry.namespace
    + "; scope=" + entry.scope + "; resolution=" + entry.resolution + "]"
}

fn lint_twin(facts) {
  let findings = [];
  let severity = "warn";
  if facts.policy == "error" {
    severity = "error";
  }
  for collision in facts.collisions {
    let message = "name `" + collision.name + "` is ambiguous in `"
      + collision.scope + "` (" + collision.namespace + "); "
      + collision.resolution + ". Owners:";
    for entry in collision.entries {
      message += "\n  - " + entry_line(entry);
    }
    findings.push(finding(
      "twin-namespace-collision",
      severity,
      collision.name,
      message,
    ));
  }
  for error in facts.read_errors {
    findings.push(finding(
      "twin-namespace-source-unavailable",
      "warn",
      facts.twin,
      error,
    ));
  }
  findings
}
