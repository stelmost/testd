//! Owner-scoped local HTTP authorization; loopback alone never grants administration.

use subtle::ConstantTimeEq;

pub(crate) fn authorized(headers: &[(String, String)], token: Option<&str>) -> bool {
	let Some(token) = token.filter(|token| !token.is_empty()) else { return false };
	let mut values = headers.iter().filter(|(name, _)| name == "authorization");
	let Some((_, value)) = values.next() else { return false };
	if values.next().is_some() {
		return false;
	}
	let Some(presented) = value.strip_prefix("Bearer ") else { return false };
	bool::from(presented.as_bytes().ct_eq(token.as_bytes()))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn authorization_requires_one_matching_nonempty_bearer_credential() {
		let header = |value: &str| ("authorization".into(), value.into());
		let valid = header("Bearer fixture-owner-token");
		assert!(authorized(std::slice::from_ref(&valid), Some("fixture-owner-token")));
		assert!(!authorized(&[], Some("fixture-owner-token")));
		assert!(!authorized(&[header("Bearer wrong")], Some("fixture-owner-token")));
		assert!(!authorized(&[header("Bearer ")], Some("")));
		assert!(!authorized(std::slice::from_ref(&valid), None));
		assert!(!authorized(&[valid.clone(), valid], Some("fixture-owner-token")));
	}
}
