//! Version constraint matching: npm ranges, Python specifiers, Terraform constraints, Java majors.

use anvil_prereqs::version::{Version, satisfies};

fn assert_table(cases: &[(&str, &str, bool)]) {
    for (found, constraint, expected) in cases {
        assert_eq!(
            satisfies(found, constraint),
            Ok(*expected),
            "`{found}` against `{constraint}` should be {expected}"
        );
    }
}

#[test]
fn npm_ranges() {
    assert_table(&[
        // npm comparators, caret, tilde, x-ranges, alternatives, hyphen ranges
        ("24.15.0", ">=24.15.0", true),
        ("24.14.9", ">=24.15.0", false),
        ("v24.16.1", ">24.15.0", true),
        ("24.15.0", ">24.15.0", false),
        ("20.0.0", "<=20", true),
        ("21.0.0", "<=20", false),
        ("v20.11.0", "^20.0.0", true),
        ("21.0.0", "^20.0.0", false),
        ("0.2.5", "^0.2.3", true),
        ("0.3.0", "^0.2.3", false),
        ("0.0.3", "^0.0.3", true),
        ("0.0.4", "^0.0.3", false),
        ("1.2.9", "~1.2.3", true),
        ("1.3.0", "~1.2.3", false),
        ("1.9.0", "~1", true),
        ("20.5.1", "20", true),
        ("21.0.0", "20", false),
        ("20.5.1", "20.x", true),
        ("20.5.1", "*", true),
        ("3.11.4", "3.11.*", true),
        ("3.12.0", "3.11.*", false),
        ("18.2.0", "18 || 20 || 22", true),
        ("19.0.0", "18 || 20", false),
        ("22.3.0", ">=18 <21 || >=22", true),
        ("21.0.0", ">=18 <21 || >=22", false),
        ("1.5.0", "1.2.3 - 2.3.4", true),
        ("2.4.0", "1.2.3 - 2.3.4", false),
        ("1.2.3-rc.1", "1.2.3", true),
    ]);
}

#[test]
fn python_specifiers_terraform_and_java() {
    assert_table(&[
        // Python specifiers
        ("3.11.2", ">=3.11,<3.13", true),
        ("3.13.0", ">=3.11,<3.13", false),
        ("3.10.9", ">=3.11,<3.13", false),
        ("3.9.1", ">= 3.8", true),
        ("3.11.5", "~=3.11.2", true),
        ("3.12.0", "~=3.11.2", false),
        ("3.14.0", "~=3.11", true),
        ("4.0.0", "~=3.11", false),
        ("3.12.1", "!=3.12.*", false),
        ("3.11.1", "!=3.12.*", true),
        ("3.11.9", "==3.11.*", true),
        // Terraform
        ("1.9.5", "1.9.5", true),
        ("1.9.6", "1.9.5", false),
        ("1.9.8", "~> 1.9.0", true),
        ("1.10.0", "~> 1.9.0", false),
        ("1.10.0", "~> 1.9", true),
        ("2.0.0", "~> 1.9", false),
        ("1.9.0", ">= 1.5.0, < 2.0.0", true),
        ("2.0.0", ">= 1.5.0, < 2.0.0", false),
        // Java majors and minimums (`java -version` reports 1.8 for Java 8)
        ("17.0.9", "17", true),
        ("21.0.1", "17", false),
        ("21.0.1", "17+", true),
        ("11.0.2", "17+", false),
        ("25", "25", true),
        ("8.0", "8", true),
        // Rust and Go minimums
        ("1.89.0", ">=1.89", true),
        ("1.88.0", ">=1.89", false),
    ]);
}

#[test]
fn unparsable_input_is_an_error_not_a_match() {
    assert!(satisfies("20.0.0", "latest").is_err());
    assert!(satisfies("not-a-version", ">=1").is_err());
    assert!(satisfies("20.0.0", ">=1 ?? 2").is_err());
}

#[test]
fn versions_parse_leniently() {
    assert_eq!(Version::parse("v24.15.0"), Some(Version(24, 15, 0)));
    assert_eq!(Version::parse("3.11"), Some(Version(3, 11, 0)));
    assert_eq!(Version::parse("1.9.5-rc1"), Some(Version(1, 9, 5)));
    assert_eq!(Version::parse("17"), Some(Version(17, 0, 0)));
    assert_eq!(Version::parse("tip"), None);
}
