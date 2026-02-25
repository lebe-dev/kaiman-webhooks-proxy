use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tera::{Context, Tera};

type HmacSha256 = Hmac<Sha256>;

/// Compute HMAC-SHA256 over `body` using `secret`, return lowercase hex string.
pub fn hmac_sha256_hex(secret: &[u8], body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// Constant-time comparison of two hex HMAC strings.
/// Normalizes both to lowercase before comparing.
pub fn verify_hmac_hex(expected: &str, actual: &str) -> bool {
    let expected = expected.to_lowercase();
    let actual = actual.to_lowercase();
    expected.as_bytes().ct_eq(actual.as_bytes()).into()
}

/// Render a Tera template with `raw` variable (for extracting hex sig from header value).
/// Returns the rendered string or an error message.
pub fn render_extract_template(template: &str, raw: &str) -> Result<String, String> {
    let mut ctx = Context::new();
    ctx.insert("raw", raw);
    render_template(template, &ctx)
}

/// Render a Tera template with `signature` variable (for formatting sig into header value).
/// Returns the rendered string or an error message.
pub fn render_sign_template(template: &str, signature: &str) -> Result<String, String> {
    let mut ctx = Context::new();
    ctx.insert("signature", signature);
    render_template(template, &ctx)
}

fn render_template(template: &str, ctx: &Context) -> Result<String, String> {
    Tera::one_off(template, ctx, false).map_err(|e| e.to_string())
}

/// Validate that a Tera template compiles without error.
pub fn validate_template(template: &str) -> Result<(), String> {
    // Parse by attempting a dummy render with empty context — parsing errors surface here.
    let mut tera = Tera::default();
    tera.add_raw_template("t", template)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known HMAC-SHA256 vector:
    // secret = "secret", body = "hello" → computed with standard tools
    const KNOWN_SECRET: &[u8] = b"secret";
    const KNOWN_BODY: &[u8] = b"hello";
    // openssl dgst -sha256 -hmac "secret" <(echo -n "hello")
    const KNOWN_HEX: &str = "88aab3ede8d3adf94d26ab90d3bafd4a2083070c3bcce9c014ee04a443847c0b";

    #[test]
    fn test_hmac_sha256_known_vector() {
        assert_eq!(hmac_sha256_hex(KNOWN_SECRET, KNOWN_BODY), KNOWN_HEX);
    }

    #[test]
    fn test_verify_hmac_hex_matching() {
        let sig = hmac_sha256_hex(KNOWN_SECRET, KNOWN_BODY);
        assert!(verify_hmac_hex(&sig, KNOWN_HEX));
    }

    #[test]
    fn test_verify_hmac_hex_mismatch() {
        assert!(!verify_hmac_hex("aaaa", "bbbb"));
    }

    #[test]
    fn test_verify_hmac_hex_case_insensitive() {
        let upper = KNOWN_HEX.to_uppercase();
        assert!(verify_hmac_hex(&upper, KNOWN_HEX));
        assert!(verify_hmac_hex(KNOWN_HEX, &upper));
    }

    #[test]
    fn test_render_extract_default_passthrough() {
        let result = render_extract_template("{{ raw }}", "sha256=abc123").unwrap();
        assert_eq!(result, "sha256=abc123");
    }

    #[test]
    fn test_render_extract_github_style() {
        let result = render_extract_template(
            r#"{{ raw | replace(from="sha256=", to="") }}"#,
            "sha256=abc123",
        )
        .unwrap();
        assert_eq!(result, "abc123");
    }

    #[test]
    fn test_render_sign_default_passthrough() {
        let result = render_sign_template("{{ signature }}", "abc123").unwrap();
        assert_eq!(result, "abc123");
    }

    #[test]
    fn test_render_sign_github_style() {
        let result = render_sign_template("sha256={{ signature }}", "abc123").unwrap();
        assert_eq!(result, "sha256=abc123");
    }

    #[test]
    fn test_render_extract_invalid_template() {
        assert!(render_extract_template("{{ unclosed", "value").is_err());
    }

    #[test]
    fn test_validate_template_valid() {
        assert!(validate_template("{{ raw }}").is_ok());
        assert!(validate_template("sha256={{ signature }}").is_ok());
    }

    #[test]
    fn test_validate_template_invalid() {
        assert!(validate_template("{{ unclosed").is_err());
    }
}
