use crate::codegen::manifest::ParamDef;

/// Validate a parameter value against its enum constraints and format.
/// Returns `Ok(())` if valid, or an `mlua::Error` with a descriptive message.
pub fn validate_param_value(
    func_name: &str,
    param: &ParamDef,
    value: &str,
) -> Result<(), mlua::Error> {
    // Check enum values first
    if let Some(ref allowed) = param.enum_values
        && !allowed.iter().any(|v| v == value)
    {
        return Err(mlua::Error::external(anyhow::anyhow!(
            "parameter '{}' for '{}': expected one of [{}], got '{}'",
            param.name,
            func_name,
            allowed.join(", "),
            value,
        )));
    }

    // Check format second
    if let Some(ref fmt) = param.format {
        validate_format(func_name, &param.name, fmt, value)?;
    }

    Ok(())
}

fn validate_format(
    func_name: &str,
    param_name: &str,
    format: &str,
    value: &str,
) -> Result<(), mlua::Error> {
    let valid = match format {
        "uuid" => is_valid_uuid(value),
        "date-time" => is_valid_date_time(value),
        "date" => is_valid_date(value),
        "email" => is_valid_email(value),
        "uri" | "url" => is_valid_uri(value),
        "ipv4" => is_valid_ipv4(value),
        "ipv6" => is_valid_ipv6(value),
        "hostname" => is_valid_hostname(value),
        "int32" => is_valid_int32(value),
        "int64" => is_valid_int64(value),
        _ => return Ok(()),
    };

    if valid {
        Ok(())
    } else {
        Err(mlua::Error::external(anyhow::anyhow!(
            "parameter '{param_name}' for '{func_name}': expected {format} format, got '{value}'",
        )))
    }
}

/// 8-4-4-4-12 hex digit pattern check.
fn is_valid_uuid(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8, 4, 4, 4, 12];
    for (part, &expected_len) in parts.iter().zip(&expected_lens) {
        if part.len() != expected_len || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

/// RFC 3339 shape: `YYYY-MM-DDTHH:MM:SS[.frac](Z|+HH:MM|-HH:MM)`.
fn is_valid_date_time(value: &str) -> bool {
    // Must have at least "YYYY-MM-DDTHH:MM:SSZ" = 20 chars
    if value.len() < 20 {
        return false;
    }

    // Split on 'T' or 't'
    let Some(t_pos) = value.find(['T', 't']) else {
        return false;
    };

    let date_part = &value[..t_pos];
    let time_and_offset = &value[t_pos + 1..];

    // Validate date portion
    if !is_valid_date(date_part) {
        return false;
    }

    // Find the timezone offset portion
    // Look for Z, +, or - after the time digits
    // Time portion is at least HH:MM:SS (8 chars)
    if time_and_offset.len() < 9 {
        // HH:MM:SS + at least one offset char
        return false;
    }

    // Find offset start: 'Z', or '+'/'-' after position 8 (after HH:MM:SS)
    let time_part;
    let offset_part;

    if let Some(z_pos) = time_and_offset.rfind(['Z', 'z']) {
        if z_pos < 8 {
            return false;
        }
        time_part = &time_and_offset[..z_pos];
        offset_part = "Z";
    } else if let Some(plus_pos) = time_and_offset.rfind('+') {
        if plus_pos < 8 {
            return false;
        }
        time_part = &time_and_offset[..plus_pos];
        offset_part = &time_and_offset[plus_pos..];
    } else if let Some(minus_pos) = time_and_offset[8..].rfind('-') {
        let actual_pos = 8 + minus_pos;
        time_part = &time_and_offset[..actual_pos];
        offset_part = &time_and_offset[actual_pos..];
    } else {
        return false;
    }

    // Validate time: HH:MM:SS[.frac]
    if time_part.len() < 8 {
        return false;
    }
    let hms = &time_part[..8];
    let bytes = hms.as_bytes();
    if !bytes[0].is_ascii_digit()
        || !bytes[1].is_ascii_digit()
        || bytes[2] != b':'
        || !bytes[3].is_ascii_digit()
        || !bytes[4].is_ascii_digit()
        || bytes[5] != b':'
        || !bytes[6].is_ascii_digit()
        || !bytes[7].is_ascii_digit()
    {
        return false;
    }

    // Optional fractional seconds
    if time_part.len() > 8 {
        let frac = &time_part[8..];
        if !frac.starts_with('.')
            || frac.len() < 2
            || !frac[1..].chars().all(|c| c.is_ascii_digit())
        {
            return false;
        }
    }

    // Validate offset
    if offset_part == "Z" || offset_part == "z" {
        return true;
    }

    // +HH:MM or -HH:MM
    if offset_part.len() != 6 {
        return false;
    }
    let ob = offset_part.as_bytes();
    (ob[0] == b'+' || ob[0] == b'-')
        && ob[1].is_ascii_digit()
        && ob[2].is_ascii_digit()
        && ob[3] == b':'
        && ob[4].is_ascii_digit()
        && ob[5].is_ascii_digit()
}

/// `YYYY-MM-DD` pattern (4-2-2 digits separated by hyphens).
fn is_valid_date(value: &str) -> bool {
    if value.len() != 10 {
        return false;
    }
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1].chars().all(|c| c.is_ascii_digit())
        && parts[2].chars().all(|c| c.is_ascii_digit())
}

/// Contains exactly one `@`, non-empty local and domain, domain contains `.`.
fn is_valid_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && !domain.is_empty() && domain.contains('.') && !domain.contains('@')
}

/// `url::Url::parse()` succeeds.
fn is_valid_uri(value: &str) -> bool {
    url::Url::parse(value).is_ok()
}

/// `std::net::Ipv4Addr::from_str()` succeeds.
fn is_valid_ipv4(value: &str) -> bool {
    value.parse::<std::net::Ipv4Addr>().is_ok()
}

/// `std::net::Ipv6Addr::from_str()` succeeds.
fn is_valid_ipv6(value: &str) -> bool {
    value.parse::<std::net::Ipv6Addr>().is_ok()
}

/// Labels separated by `.`, each 1-63 chars, alphanumeric + hyphens only,
/// no leading/trailing hyphens, total <= 253.
fn is_valid_hostname(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 {
        return false;
    }
    for label in value.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }
    true
}

/// Parse as i64, check `[-2^31, 2^31-1]` range.
fn is_valid_int32(value: &str) -> bool {
    value
        .parse::<i64>()
        .is_ok_and(|n| (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&n))
}

/// Parse as i64 succeeds.
fn is_valid_int64(value: &str) -> bool {
    value.parse::<i64>().is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::codegen::manifest::{ParamLocation, ParamType};

    fn make_param(
        name: &str,
        enum_values: Option<Vec<String>>,
        format: Option<String>,
    ) -> ParamDef {
        ParamDef {
            name: name.to_string(),
            location: ParamLocation::Query,
            param_type: ParamType::String,
            required: true,
            description: None,
            default: None,
            enum_values,
            format,
            frozen_value: None,
        }
    }

    // -------------------------------------------------------
    // Enum validation
    // -------------------------------------------------------

    #[test]
    fn enum_valid_value_passes() {
        let param = make_param(
            "status",
            Some(vec!["active".to_string(), "inactive".to_string()]),
            None,
        );
        assert!(validate_param_value("list_users", &param, "active").is_ok());
    }

    #[test]
    fn enum_invalid_value_returns_error_with_details() {
        let param = make_param(
            "status",
            Some(vec!["active".to_string(), "inactive".to_string()]),
            None,
        );
        let err = validate_param_value("list_users", &param, "deleted")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("list_users"),
            "error should contain func name: {err}"
        );
        assert!(
            err.contains("status"),
            "error should contain param name: {err}"
        );
        assert!(
            err.contains("deleted"),
            "error should contain actual value: {err}"
        );
        assert!(
            err.contains("active"),
            "error should contain allowed values: {err}"
        );
    }

    // -------------------------------------------------------
    // No constraints
    // -------------------------------------------------------

    #[test]
    fn no_constraints_passes_any_value() {
        let param = make_param("anything", None, None);
        assert!(validate_param_value("func", &param, "literally anything!").is_ok());
    }

    // -------------------------------------------------------
    // UUID
    // -------------------------------------------------------

    #[test]
    fn uuid_valid_lowercase() {
        let param = make_param("id", None, Some("uuid".to_string()));
        assert!(
            validate_param_value("get_item", &param, "550e8400-e29b-41d4-a716-446655440000")
                .is_ok()
        );
    }

    #[test]
    fn uuid_valid_uppercase() {
        let param = make_param("id", None, Some("uuid".to_string()));
        assert!(
            validate_param_value("get_item", &param, "550E8400-E29B-41D4-A716-446655440000")
                .is_ok()
        );
    }

    #[test]
    fn uuid_invalid_format() {
        let param = make_param("id", None, Some("uuid".to_string()));
        assert!(validate_param_value("get_item", &param, "not-a-uuid").is_err());
    }

    #[test]
    fn uuid_invalid_too_short() {
        let param = make_param("id", None, Some("uuid".to_string()));
        assert!(validate_param_value("get_item", &param, "550e8400-e29b-41d4-a716").is_err());
    }

    // -------------------------------------------------------
    // date-time
    // -------------------------------------------------------

    #[test]
    fn datetime_valid_z() {
        let param = make_param("ts", None, Some("date-time".to_string()));
        assert!(validate_param_value("f", &param, "2024-01-15T08:30:00Z").is_ok());
    }

    #[test]
    fn datetime_valid_positive_offset() {
        let param = make_param("ts", None, Some("date-time".to_string()));
        assert!(validate_param_value("f", &param, "2024-01-15T08:30:00+05:30").is_ok());
    }

    #[test]
    fn datetime_valid_fractional_seconds() {
        let param = make_param("ts", None, Some("date-time".to_string()));
        assert!(validate_param_value("f", &param, "2024-01-15T08:30:00.123Z").is_ok());
    }

    #[test]
    fn datetime_invalid_just_a_date() {
        let param = make_param("ts", None, Some("date-time".to_string()));
        assert!(validate_param_value("f", &param, "2024-01-15").is_err());
    }

    #[test]
    fn datetime_invalid_garbage() {
        let param = make_param("ts", None, Some("date-time".to_string()));
        assert!(validate_param_value("f", &param, "not-a-datetime").is_err());
    }

    // -------------------------------------------------------
    // date
    // -------------------------------------------------------

    #[test]
    fn date_valid() {
        let param = make_param("d", None, Some("date".to_string()));
        assert!(validate_param_value("f", &param, "2024-01-15").is_ok());
    }

    #[test]
    fn date_invalid_wrong_format() {
        let param = make_param("d", None, Some("date".to_string()));
        assert!(validate_param_value("f", &param, "01-15-2024").is_err());
    }

    // -------------------------------------------------------
    // email
    // -------------------------------------------------------

    #[test]
    fn email_valid() {
        let param = make_param("e", None, Some("email".to_string()));
        assert!(validate_param_value("f", &param, "user@example.com").is_ok());
    }

    #[test]
    fn email_invalid_no_at() {
        let param = make_param("e", None, Some("email".to_string()));
        assert!(validate_param_value("f", &param, "userexample.com").is_err());
    }

    #[test]
    fn email_invalid_no_domain_dot() {
        let param = make_param("e", None, Some("email".to_string()));
        assert!(validate_param_value("f", &param, "user@localhost").is_err());
    }

    // -------------------------------------------------------
    // uri / url
    // -------------------------------------------------------

    #[test]
    fn uri_valid_https() {
        let param = make_param("u", None, Some("uri".to_string()));
        assert!(validate_param_value("f", &param, "https://example.com/path?q=1").is_ok());
    }

    #[test]
    fn uri_invalid() {
        let param = make_param("u", None, Some("uri".to_string()));
        assert!(validate_param_value("f", &param, "not a url").is_err());
    }

    #[test]
    fn url_format_valid() {
        let param = make_param("u", None, Some("url".to_string()));
        assert!(validate_param_value("f", &param, "https://example.com").is_ok());
    }

    // -------------------------------------------------------
    // ipv4
    // -------------------------------------------------------

    #[test]
    fn ipv4_valid() {
        let param = make_param("ip", None, Some("ipv4".to_string()));
        assert!(validate_param_value("f", &param, "192.168.1.1").is_ok());
    }

    #[test]
    fn ipv4_invalid_out_of_range() {
        let param = make_param("ip", None, Some("ipv4".to_string()));
        assert!(validate_param_value("f", &param, "999.999.999.999").is_err());
    }

    // -------------------------------------------------------
    // ipv6
    // -------------------------------------------------------

    #[test]
    fn ipv6_valid_loopback() {
        let param = make_param("ip", None, Some("ipv6".to_string()));
        assert!(validate_param_value("f", &param, "::1").is_ok());
    }

    #[test]
    fn ipv6_invalid() {
        let param = make_param("ip", None, Some("ipv6".to_string()));
        assert!(validate_param_value("f", &param, "not-ipv6").is_err());
    }

    // -------------------------------------------------------
    // hostname
    // -------------------------------------------------------

    #[test]
    fn hostname_valid() {
        let param = make_param("h", None, Some("hostname".to_string()));
        assert!(validate_param_value("f", &param, "api.example.com").is_ok());
    }

    #[test]
    fn hostname_invalid_underscore() {
        let param = make_param("h", None, Some("hostname".to_string()));
        assert!(validate_param_value("f", &param, "invalid_host.com").is_err());
    }

    #[test]
    fn hostname_invalid_label_too_long() {
        let param = make_param("h", None, Some("hostname".to_string()));
        let long_label = "a".repeat(64);
        let hostname = format!("{long_label}.com");
        assert!(validate_param_value("f", &param, &hostname).is_err());
    }

    // -------------------------------------------------------
    // int32
    // -------------------------------------------------------

    #[test]
    fn int32_valid_positive() {
        let param = make_param("n", None, Some("int32".to_string()));
        assert!(validate_param_value("f", &param, "42").is_ok());
    }

    #[test]
    fn int32_valid_min() {
        let param = make_param("n", None, Some("int32".to_string()));
        assert!(validate_param_value("f", &param, "-2147483648").is_ok());
    }

    #[test]
    fn int32_valid_max() {
        let param = make_param("n", None, Some("int32".to_string()));
        assert!(validate_param_value("f", &param, "2147483647").is_ok());
    }

    #[test]
    fn int32_invalid_overflow() {
        let param = make_param("n", None, Some("int32".to_string()));
        assert!(validate_param_value("f", &param, "2147483648").is_err());
    }

    #[test]
    fn int32_invalid_underflow() {
        let param = make_param("n", None, Some("int32".to_string()));
        assert!(validate_param_value("f", &param, "-2147483649").is_err());
    }

    #[test]
    fn int32_invalid_non_numeric() {
        let param = make_param("n", None, Some("int32".to_string()));
        assert!(validate_param_value("f", &param, "abc").is_err());
    }

    // -------------------------------------------------------
    // int64
    // -------------------------------------------------------

    #[test]
    fn int64_valid_max() {
        let param = make_param("n", None, Some("int64".to_string()));
        assert!(validate_param_value("f", &param, "9223372036854775807").is_ok());
    }

    #[test]
    fn int64_invalid_non_numeric() {
        let param = make_param("n", None, Some("int64".to_string()));
        assert!(validate_param_value("f", &param, "xyz").is_err());
    }

    // -------------------------------------------------------
    // Unknown format
    // -------------------------------------------------------

    #[test]
    fn unknown_format_passes_any_value() {
        let param = make_param("x", None, Some("custom-thing".to_string()));
        assert!(validate_param_value("f", &param, "literally anything").is_ok());
    }

    // -------------------------------------------------------
    // Edge cases
    // -------------------------------------------------------

    #[test]
    fn empty_string_no_constraints_passes() {
        let param = make_param("x", None, None);
        assert!(validate_param_value("f", &param, "").is_ok());
    }

    #[test]
    fn int32_empty_string_fails() {
        let param = make_param("n", None, Some("int32".to_string()));
        assert!(validate_param_value("f", &param, "").is_err());
    }

    #[test]
    fn datetime_valid_negative_offset() {
        let param = make_param("ts", None, Some("date-time".to_string()));
        assert!(validate_param_value("f", &param, "2024-01-15T08:30:00-05:00").is_ok());
    }

    #[test]
    fn datetime_valid_lowercase_z() {
        let param = make_param("ts", None, Some("date-time".to_string()));
        assert!(validate_param_value("f", &param, "2024-01-15T10:30:00z").is_ok());
    }
}
