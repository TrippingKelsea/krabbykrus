//! Permission evaluation engine for rockbot-credentials.
//!
//! Evaluates incoming requests against configured permission rules
//! to determine the appropriate access level (Allow/AllowHIL/AllowHIL2FA/Deny).
//!
//! # Pattern Matching
//!
//! Path patterns use glob syntax:
//! - `*` matches any sequence of characters except `/`
//! - `**` matches any sequence of characters including `/`
//! - `?` matches any single character except `/`
//!
//! # Evaluation Order
//!
//! When multiple rules match, the most specific rule wins:
//! 1. Exact path matches take precedence over patterns
//! 2. Longer patterns take precedence over shorter ones
//! 3. Method-specific rules take precedence over wildcard method rules
//! 4. If still tied, the most restrictive permission wins (Deny > AllowHIL2FA > AllowHIL > Allow)

use std::collections::HashMap;

use uuid::Uuid;

use crate::types::{HttpMethod, Permission, PermissionLevel};

/// Permission evaluator for request authorization.
pub struct PermissionEvaluator {
    /// Permissions indexed by endpoint ID.
    permissions: HashMap<Uuid, Vec<Permission>>,
}

impl PermissionEvaluator {
    /// Creates a new permission evaluator with no rules.
    pub fn new() -> Self {
        Self {
            permissions: HashMap::new(),
        }
    }

    /// Creates a permission evaluator from a list of permissions.
    pub fn from_permissions(permissions: Vec<Permission>) -> Self {
        let mut evaluator = Self::new();
        for perm in permissions {
            evaluator.add_permission(perm);
        }
        evaluator
    }

    /// Adds a permission rule.
    pub fn add_permission(&mut self, permission: Permission) {
        self.permissions
            .entry(permission.endpoint_id)
            .or_default()
            .push(permission);
    }

    /// Removes a permission rule by ID.
    pub fn remove_permission(&mut self, permission_id: Uuid) -> bool {
        for perms in self.permissions.values_mut() {
            if let Some(pos) = perms.iter().position(|p| p.id == permission_id) {
                perms.remove(pos);
                return true;
            }
        }
        false
    }

    /// Lists all permissions for an endpoint.
    pub fn list_permissions(&self, endpoint_id: Uuid) -> Vec<&Permission> {
        self.permissions
            .get(&endpoint_id)
            .map(|perms| perms.iter().collect())
            .unwrap_or_default()
    }

    /// Evaluates permission for a request.
    ///
    /// Returns the permission level and the matching rule (if any).
    /// If no rule matches, returns `Deny` as the default.
    pub fn evaluate(&self, endpoint_id: Uuid, method: HttpMethod, path: &str) -> PermissionResult {
        let Some(perms) = self.permissions.get(&endpoint_id) else {
            return PermissionResult {
                level: PermissionLevel::Deny,
                matched_rule: None,
                reason: "no permissions configured for endpoint".to_string(),
            };
        };

        // Find all matching rules
        let mut matches: Vec<(&Permission, MatchScore)> = perms
            .iter()
            .filter_map(|perm| self.matches(perm, method, path).map(|score| (perm, score)))
            .collect();

        if matches.is_empty() {
            return PermissionResult {
                level: PermissionLevel::Deny,
                matched_rule: None,
                reason: "no matching permission rule".to_string(),
            };
        }

        // Sort by specificity (highest first)
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        // If there's a tie in specificity, use the most restrictive permission
        let best_score = matches[0].1;
        let tied_matches: Vec<_> = matches
            .iter()
            .take_while(|(_, score)| *score == best_score)
            .collect();

        // Among tied matches, pick the most restrictive
        #[allow(clippy::unwrap_used)]
        // tied_matches is non-empty (matches.is_empty() checked above)
        let (best_perm, _) = tied_matches
            .into_iter()
            .max_by_key(|(perm, _)| restriction_level(perm.permission_level))
            .unwrap();

        PermissionResult {
            level: best_perm.permission_level,
            matched_rule: Some(best_perm.id),
            reason: format!(
                "matched rule: {} ({})",
                best_perm.path_pattern,
                best_perm.method.map_or("*", |m| m.as_str())
            ),
        }
    }

    /// Checks if a permission matches a request, returning a match score if it does.
    #[allow(clippy::unused_self)]
    fn matches(&self, perm: &Permission, method: HttpMethod, path: &str) -> Option<MatchScore> {
        // Check method constraint
        if let Some(perm_method) = perm.method {
            if perm_method != method {
                return None;
            }
        }

        // Check path pattern
        if !pattern_matches(&perm.path_pattern, path) {
            return None;
        }

        // Calculate match score
        let is_exact = !perm.path_pattern.contains('*') && !perm.path_pattern.contains('?');
        let pattern_len = perm.path_pattern.len();
        let has_method_constraint = perm.method.is_some();

        Some(MatchScore {
            is_exact,
            pattern_len,
            has_method_constraint,
        })
    }
}

impl Default for PermissionEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of permission evaluation.
#[derive(Debug, Clone)]
pub struct PermissionResult {
    /// The evaluated permission level.
    pub level: PermissionLevel,
    /// ID of the rule that matched, if any.
    pub matched_rule: Option<Uuid>,
    /// Human-readable reason for the decision.
    pub reason: String,
}

impl PermissionResult {
    /// Returns whether the request is allowed (any level except Deny).
    pub fn is_allowed(&self) -> bool {
        self.level.allows_execution()
    }

    /// Returns whether human approval is required.
    pub fn requires_approval(&self) -> bool {
        self.level.requires_approval()
    }

    /// Returns whether 2FA is required.
    pub fn requires_2fa(&self) -> bool {
        self.level.requires_2fa()
    }
}

/// Score for ranking permission matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MatchScore {
    /// Exact matches rank highest.
    is_exact: bool,
    /// Longer patterns are more specific.
    pattern_len: usize,
    /// Method-specific rules rank higher.
    has_method_constraint: bool,
}

/// Returns a numeric restriction level for comparison (higher = more restrictive).
fn restriction_level(level: PermissionLevel) -> u8 {
    match level {
        PermissionLevel::Allow => 0,
        PermissionLevel::AllowHil => 1,
        PermissionLevel::AllowHil2fa => 2,
        PermissionLevel::Deny => 3,
    }
}

/// Matches a glob pattern against a path.
///
/// Supports:
/// - `*` matches any sequence except `/`
/// - `**` matches any sequence including `/`
/// - `?` matches any single character except `/`
fn pattern_matches(pattern: &str, path: &str) -> bool {
    pattern_match_recursive(pattern, path)
}

/// Recursive glob matching with memoization via simple recursion.
#[allow(clippy::unwrap_used)] // all unwrap()s on chars().next() are guarded by !path.is_empty() or equivalent checks
fn pattern_match_recursive(pattern: &str, path: &str) -> bool {
    let mut pattern = pattern;
    let mut path = path;

    // Track backtrack points for `*` (single star)
    let mut star_pattern: Option<&str> = None;
    let mut star_path: Option<&str> = None;

    while !path.is_empty() {
        if pattern.starts_with("**") {
            // `**` matches anything including `/`
            pattern = &pattern[2..];
            // Skip optional `/` after `**`
            if pattern.starts_with('/') {
                pattern = &pattern[1..];
            }
            // `**` at end matches everything
            if pattern.is_empty() {
                return true;
            }
            // Try matching rest of pattern at every position in path
            for i in 0..=path.len() {
                if pattern_match_recursive(pattern, &path[i..]) {
                    return true;
                }
            }
            return false;
        } else if let Some(pc) = pattern.chars().next() {
            let path_char = path.chars().next().unwrap();

            match pc {
                '?' if path_char != '/' => {
                    // `?` matches any single char except `/`
                    pattern = &pattern[1..];
                    path = &path[path_char.len_utf8()..];
                }
                '*' => {
                    // Single `*` - matches any sequence except `/`
                    // Remember position for backtracking
                    star_pattern = Some(&pattern[1..]);
                    star_path = Some(path);
                    pattern = &pattern[1..];
                }
                c if c == path_char => {
                    // Exact character match
                    pattern = &pattern[c.len_utf8()..];
                    path = &path[path_char.len_utf8()..];
                }
                _ => {
                    // No match - try backtracking to last `*`
                    if let (Some(sp), Some(st)) = (star_pattern, star_path) {
                        let st_char = st.chars().next().unwrap();
                        if st_char == '/' {
                            // Single `*` cannot match `/`
                            return false;
                        }
                        // Consume one more char with the `*`
                        star_path = Some(&st[st_char.len_utf8()..]);
                        path = star_path.unwrap();
                        pattern = sp;
                    } else {
                        return false;
                    }
                }
            }
        } else {
            // Pattern exhausted but path remains - try backtracking
            if let (Some(sp), Some(st)) = (star_pattern, star_path) {
                let st_char = st.chars().next().unwrap();
                if st_char == '/' {
                    return false;
                }
                star_path = Some(&st[st_char.len_utf8()..]);
                path = star_path.unwrap();
                pattern = sp;
            } else {
                return false;
            }
        }
    }

    // Path exhausted - remaining pattern should all be `*` or `**`
    while pattern.starts_with('*') {
        pattern = &pattern[1..];
    }

    pattern.is_empty()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::Utc;

    fn make_permission(
        endpoint_id: Uuid,
        path_pattern: &str,
        method: Option<HttpMethod>,
        level: PermissionLevel,
    ) -> Permission {
        Permission {
            id: Uuid::new_v4(),
            endpoint_id,
            path_pattern: path_pattern.to_string(),
            method,
            permission_level: level,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_pattern_exact_match() {
        assert!(pattern_matches("/api/states", "/api/states"));
        assert!(!pattern_matches("/api/states", "/api/states/extra"));
        assert!(!pattern_matches("/api/states/extra", "/api/states"));
    }

    #[test]
    fn test_pattern_single_star() {
        assert!(pattern_matches("/api/*/states", "/api/v1/states"));
        assert!(pattern_matches("/api/*/states", "/api/foo/states"));
        assert!(!pattern_matches("/api/*/states", "/api/foo/bar/states"));
        assert!(!pattern_matches("/api/*", "/api/foo/bar"));
    }

    #[test]
    fn test_pattern_double_star() {
        assert!(pattern_matches("/api/**", "/api/foo"));
        assert!(pattern_matches("/api/**", "/api/foo/bar"));
        assert!(pattern_matches("/api/**", "/api/foo/bar/baz"));
        assert!(pattern_matches("/**", "/anything/at/all"));
        assert!(pattern_matches("/api/**/states", "/api/foo/states"));
        assert!(pattern_matches("/api/**/states", "/api/foo/bar/states"));
    }

    #[test]
    fn test_pattern_question_mark() {
        assert!(pattern_matches("/api/v?/states", "/api/v1/states"));
        assert!(pattern_matches("/api/v?/states", "/api/vX/states"));
        assert!(!pattern_matches("/api/v?/states", "/api/v12/states"));
        assert!(!pattern_matches("/api/v?/states", "/api/v/states"));
    }

    #[test]
    fn test_evaluate_no_rules() {
        let evaluator = PermissionEvaluator::new();
        let result = evaluator.evaluate(Uuid::new_v4(), HttpMethod::Get, "/api/states");
        assert_eq!(result.level, PermissionLevel::Deny);
        assert!(result.matched_rule.is_none());
    }

    #[test]
    fn test_evaluate_single_rule() {
        let endpoint_id = Uuid::new_v4();
        let mut evaluator = PermissionEvaluator::new();

        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/states",
            Some(HttpMethod::Get),
            PermissionLevel::Allow,
        ));

        let result = evaluator.evaluate(endpoint_id, HttpMethod::Get, "/api/states");
        assert_eq!(result.level, PermissionLevel::Allow);
        assert!(result.matched_rule.is_some());

        // Wrong method
        let result = evaluator.evaluate(endpoint_id, HttpMethod::Post, "/api/states");
        assert_eq!(result.level, PermissionLevel::Deny);

        // Wrong path
        let result = evaluator.evaluate(endpoint_id, HttpMethod::Get, "/api/other");
        assert_eq!(result.level, PermissionLevel::Deny);
    }

    #[test]
    fn test_evaluate_wildcard_method() {
        let endpoint_id = Uuid::new_v4();
        let mut evaluator = PermissionEvaluator::new();

        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/states",
            None, // Any method
            PermissionLevel::Allow,
        ));

        let result = evaluator.evaluate(endpoint_id, HttpMethod::Get, "/api/states");
        assert_eq!(result.level, PermissionLevel::Allow);

        let result = evaluator.evaluate(endpoint_id, HttpMethod::Post, "/api/states");
        assert_eq!(result.level, PermissionLevel::Allow);
    }

    #[test]
    fn test_evaluate_method_specific_wins() {
        let endpoint_id = Uuid::new_v4();
        let mut evaluator = PermissionEvaluator::new();

        // General rule: Allow all methods
        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/states",
            None,
            PermissionLevel::Allow,
        ));

        // Specific rule: Deny POST
        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/states",
            Some(HttpMethod::Post),
            PermissionLevel::Deny,
        ));

        // GET should be allowed
        let result = evaluator.evaluate(endpoint_id, HttpMethod::Get, "/api/states");
        assert_eq!(result.level, PermissionLevel::Allow);

        // POST should be denied (specific rule wins)
        let result = evaluator.evaluate(endpoint_id, HttpMethod::Post, "/api/states");
        assert_eq!(result.level, PermissionLevel::Deny);
    }

    #[test]
    fn test_evaluate_more_specific_pattern_wins() {
        let endpoint_id = Uuid::new_v4();
        let mut evaluator = PermissionEvaluator::new();

        // General rule
        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/**",
            None,
            PermissionLevel::Allow,
        ));

        // More specific rule
        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/services/**",
            None,
            PermissionLevel::AllowHil,
        ));

        // Even more specific
        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/services/light/turn_on",
            None,
            PermissionLevel::AllowHil2fa,
        ));

        // General path -> Allow
        let result = evaluator.evaluate(endpoint_id, HttpMethod::Get, "/api/states");
        assert_eq!(result.level, PermissionLevel::Allow);

        // Services path -> AllowHIL
        let result =
            evaluator.evaluate(endpoint_id, HttpMethod::Post, "/api/services/switch/toggle");
        assert_eq!(result.level, PermissionLevel::AllowHil);

        // Exact path -> AllowHIL2FA
        let result =
            evaluator.evaluate(endpoint_id, HttpMethod::Post, "/api/services/light/turn_on");
        assert_eq!(result.level, PermissionLevel::AllowHil2fa);
    }

    #[test]
    fn test_evaluate_tied_rules_most_restrictive_wins() {
        let endpoint_id = Uuid::new_v4();
        let mut evaluator = PermissionEvaluator::new();

        // Two rules with same pattern and method constraint
        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/states",
            Some(HttpMethod::Get),
            PermissionLevel::Allow,
        ));

        evaluator.add_permission(make_permission(
            endpoint_id,
            "/api/states",
            Some(HttpMethod::Get),
            PermissionLevel::AllowHil,
        ));

        // Should pick the more restrictive one
        let result = evaluator.evaluate(endpoint_id, HttpMethod::Get, "/api/states");
        assert_eq!(result.level, PermissionLevel::AllowHil);
    }

    #[test]
    fn test_list_permissions() {
        let endpoint_id = Uuid::new_v4();
        let mut evaluator = PermissionEvaluator::new();

        let p1 = make_permission(endpoint_id, "/api/states", None, PermissionLevel::Allow);
        let p2 = make_permission(
            endpoint_id,
            "/api/services/**",
            None,
            PermissionLevel::AllowHil,
        );

        evaluator.add_permission(p1.clone());
        evaluator.add_permission(p2.clone());

        let perms = evaluator.list_permissions(endpoint_id);
        assert_eq!(perms.len(), 2);
    }

    #[test]
    fn test_remove_permission() {
        let endpoint_id = Uuid::new_v4();
        let mut evaluator = PermissionEvaluator::new();

        let p1 = make_permission(endpoint_id, "/api/states", None, PermissionLevel::Allow);
        let p1_id = p1.id;

        evaluator.add_permission(p1);

        assert!(evaluator.remove_permission(p1_id));
        assert!(!evaluator.remove_permission(p1_id)); // Already removed

        let perms = evaluator.list_permissions(endpoint_id);
        assert!(perms.is_empty());
    }

    #[test]
    fn test_permission_result_helpers() {
        let result = PermissionResult {
            level: PermissionLevel::AllowHil,
            matched_rule: Some(Uuid::new_v4()),
            reason: "test".to_string(),
        };

        assert!(result.is_allowed());
        assert!(result.requires_approval());
        assert!(!result.requires_2fa());

        let result = PermissionResult {
            level: PermissionLevel::Deny,
            matched_rule: None,
            reason: "test".to_string(),
        };

        assert!(!result.is_allowed());
        assert!(!result.requires_approval());
    }
}
