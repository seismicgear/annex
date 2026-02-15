use annex_types::ServerPolicy;
use annex_vrp::{ServerPolicyRoot, VrpAnchorSnapshot};

#[test]
fn test_policy_to_root_to_snapshot() {
    let principles = vec!["Transparency".to_string(), "User Sovereignty".to_string()];
    let prohibited_actions = vec!["Data Selling".to_string(), "Censorship".to_string()];

    let mut policy = ServerPolicy::default();
    policy.principles = principles.clone();
    policy.prohibited_actions = prohibited_actions.clone();

    // 1. Convert to ServerPolicyRoot
    let root = ServerPolicyRoot::from_policy(&policy);

    assert_eq!(root.principles, principles);
    assert_eq!(root.prohibited_actions, prohibited_actions);

    // 2. Convert to VrpAnchorSnapshot
    let snapshot = root.to_anchor_snapshot();

    // 3. Compare with manually created snapshot
    let expected_snapshot = VrpAnchorSnapshot::new(&principles, &prohibited_actions);

    assert_eq!(snapshot.principles_hash, expected_snapshot.principles_hash);
    assert_eq!(
        snapshot.prohibited_actions_hash,
        expected_snapshot.prohibited_actions_hash
    );

    // Note: timestamps will differ, so we don't compare them.
}

#[test]
fn test_determinism_irrespective_of_order() {
    let principles1 = vec!["A".to_string(), "B".to_string()];
    let principles2 = vec!["B".to_string(), "A".to_string()];

    let root1 = ServerPolicyRoot::new(principles1.clone(), vec![]);
    let root2 = ServerPolicyRoot::new(principles2.clone(), vec![]);

    let snap1 = root1.to_anchor_snapshot();
    let snap2 = root2.to_anchor_snapshot();

    assert_eq!(snap1.principles_hash, snap2.principles_hash);
}
