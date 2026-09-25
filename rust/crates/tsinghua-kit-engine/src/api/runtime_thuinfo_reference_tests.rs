use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Vector {
    input: String,
    expected: String,
}

#[test]
fn backend_repair_thuinfo_runtime_uses_actual_reference_url_outputs() {
    let cases: Vec<Vector> =
        serde_json::from_str(include_str!("../portal_thuinfo_fixtures.json")).unwrap();
    for case in cases {
        assert_eq!(
            portal_oauth_redirect_url(&Url::parse(&case.input).unwrap())
                .unwrap()
                .as_str(),
            case.expected
        );
    }
}

#[test]
fn backend_repair_thuinfo_server_selected_raw_broker_url_stays_unchanged() {
    let raw=Url::parse("https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=https&host=info.tsinghua.edu.cn&port=443&uri=/f/info/gxfw_fg/common/index?ticket=FIXTURE%2Btoken&lang=zh").unwrap();
    assert!(validated_server_selected_portal_oauth(
        &raw,
        &Url::parse(INFO_DIRECT_ORIGIN).unwrap()
    ));
    assert_eq!(portal_oauth_redirect_url(&raw).unwrap(), raw);
}

#[test]
fn backend_repair_thuinfo_broker_does_not_allow_outer_authority_override() {
    for input in [
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=https&host=info.tsinghua.edu.cn&port=443&uri=/f/index&host=evil.invalid",
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=https&host=evil.invalid&port=443&uri=/f/index",
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=https&host=info.tsinghua.edu.cn&port=443&uri=//evil.invalid/f/index",
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=https&host=info.tsinghua.edu.cn&port=443&uri=%2F%2Fevil.invalid%2F",
    ] {
        assert!(portal_oauth_redirect_url(&Url::parse(input).unwrap()).is_err());
    }
}
