//! Integration tests for multi-language AST extractors and parallel throughput.

use std::path::Path;
use std::time::Instant;

use anvil_ast::{analyze_file, analyze_files_parallel, detect_language};
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeKind, Language, SymbolKind};

#[test]
fn detects_all_supported_languages() {
    assert_eq!(detect_language(Path::new("a.ts")), Language::TypeScript);
    assert_eq!(detect_language(Path::new("a.tsx")), Language::Tsx);
    assert_eq!(detect_language(Path::new("a.js")), Language::JavaScript);
    assert_eq!(detect_language(Path::new("a.jsx")), Language::Jsx);
    assert_eq!(detect_language(Path::new("a.html")), Language::Html);
    assert_eq!(detect_language(Path::new("a.css")), Language::Css);
    assert_eq!(detect_language(Path::new("a.scss")), Language::Scss);
    assert_eq!(detect_language(Path::new("a.py")), Language::Python);
    assert_eq!(detect_language(Path::new("a.kt")), Language::Kotlin);
    assert_eq!(detect_language(Path::new("a.dart")), Language::Dart);
    assert_eq!(detect_language(Path::new("a.rs")), Language::Rust);
}

#[test]
fn extracts_tsx_jsx_props_and_typescript_symbols() {
    let cfg = AnvilConfig::default();
    let src = r#"
import { Button } from './Button';
export interface CardProps { title: string; }
export type CardId = string;
export function ProfileCard({ title }: CardProps) {
    return <Button variant="primary" onClick={handleSave} />;
}
"#;
    let facts = analyze_file(Path::new("src/ProfileCard.tsx"), src, &cfg).expect("tsx");
    assert_eq!(facts.language, Language::Tsx);
    assert!(!facts.blake3_hash.is_empty());

    assert!(facts.symbols.iter().any(|s| s.name == "CardProps" && s.kind == SymbolKind::Interface));
    assert!(facts.symbols.iter().any(|s| s.name == "CardId" && s.kind == SymbolKind::TypeAlias));
    assert!(facts.symbols.iter().any(|s| s.name == "ProfileCard" && s.kind == SymbolKind::Component));

    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "./Button"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::UsesProp && e.target_path_or_symbol == "Button.variant"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::UsesProp && e.target_path_or_symbol == "Button.onClick"));
}

#[test]
fn extracts_angular_component_and_html_template_and_nestjs() {
    let cfg = AnvilConfig::default();
    let ng_ts = r#"
import { Component } from '@angular/core';
@Component({
  selector: 'app-user-card',
  templateUrl: './user-card.component.html'
})
export class UserCardComponent {}
"#;
    let ts_facts = analyze_file(Path::new("src/user-card.component.ts"), ng_ts, &cfg).expect("ng");
    assert!(ts_facts.symbols.iter().any(|s| s.name == "app-user-card" && s.kind == SymbolKind::Component));
    assert!(ts_facts.edges.iter().any(|e| e.kind == EdgeKind::RendersTemplate && e.target_path_or_symbol == "./user-card.component.html"));

    let ng_html = r#"<section><app-user-card [user]="activeUser" (selected)="onSelect($event)"></app-user-card></section>"#;
    let html_facts = analyze_file(Path::new("src/user-card.component.html"), ng_html, &cfg).expect("html");
    assert!(html_facts.edges.iter().any(|e| e.kind == EdgeKind::RendersTemplate && e.target_path_or_symbol == "app-user-card"));
    assert!(html_facts.edges.iter().any(|e| e.kind == EdgeKind::UsesProp && e.target_path_or_symbol == "app-user-card.user"));
    assert!(html_facts.edges.iter().any(|e| e.kind == EdgeKind::Calls && e.target_path_or_symbol == "onSelect"));

    let nest = r#"
@Injectable()
export class AuthService {
  constructor(private readonly repo: UserRepo) {}
}
@Module({
  imports: [UsersModule],
  providers: [AuthService],
  controllers: [AuthController]
})
export class AuthModule {}
"#;
    let nest_facts = analyze_file(Path::new("src/auth.module.ts"), nest, &cfg).expect("nest");
    assert!(nest_facts.symbols.iter().any(|s| s.name == "AuthModule" && s.kind == SymbolKind::Module));
    assert!(nest_facts.edges.iter().any(|e| e.kind == EdgeKind::Injects && e.target_path_or_symbol == "UserRepo"));
    assert!(nest_facts.edges.iter().any(|e| e.kind == EdgeKind::Provides && e.target_path_or_symbol == "AuthService"));
    assert!(nest_facts.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "UsersModule"));
}

#[test]
fn extracts_python_kotlin_dart_and_css() {
    let cfg = AnvilConfig::default();

    let py = "from core.auth import BaseUser\nclass Admin(BaseUser):\n    def verify(self):\n        check_token()\n";
    let py_f = analyze_file(Path::new("app/admin.py"), py, &cfg).expect("py");
    assert!(py_f.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "core.auth"));
    assert!(py_f.edges.iter().any(|e| e.kind == EdgeKind::Extends && e.target_path_or_symbol == "BaseUser"));
    assert!(py_f.symbols.iter().any(|s| s.qualified_name == "Admin.verify" && s.kind == SymbolKind::Method));

    let kt = "import androidx.compose.runtime.Composable\n@Composable\nfun Greeting(name: String) { Text(name) }\nclass RepoImpl : BaseRepo(), IRepo {}\n";
    let kt_f = analyze_file(Path::new("app/Greeting.kt"), kt, &cfg).expect("kt");
    assert!(kt_f.symbols.iter().any(|s| s.name == "Greeting" && s.kind == SymbolKind::Component));
    assert!(kt_f.edges.iter().any(|e| e.kind == EdgeKind::Extends && e.target_path_or_symbol == "BaseRepo"));
    assert!(kt_f.edges.iter().any(|e| e.kind == EdgeKind::Implements && e.target_path_or_symbol == "IRepo"));

    let dart = "import 'package:flutter/material.dart';\nclass HomeCard extends StatelessWidget {\n  Widget build(BuildContext context) => Container();\n}\n";
    let dart_f = analyze_file(Path::new("lib/home_card.dart"), dart, &cfg).expect("dart");
    assert!(dart_f.symbols.iter().any(|s| s.name == "HomeCard" && s.kind == SymbolKind::Component));
    assert!(dart_f.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "package:flutter/material.dart"));

    let css = "@import './tokens.css';\n:root { --brand: #111; }\n.btn-primary { color: var(--brand); }\n";
    let css_f = analyze_file(Path::new("src/styles.css"), css, &cfg).expect("css");
    assert!(css_f.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "./tokens.css"));
    assert!(css_f.symbols.iter().any(|s| s.name == ".btn-primary"));
    assert!(css_f.symbols.iter().any(|s| s.name == "--brand"));
}

#[test]
fn parses_50_files_in_parallel_under_200ms() {
    let cfg = AnvilConfig::default();
    let files: Vec<(String, String)> = (0..50)
        .map(|i| {
            match i % 5 {
                0 => (format!("src/mod_{i}.ts"), format!("import {{ x }} from './x';\nexport function fn{i}(v: number) {{ return v + x; }}\n")),
                1 => (format!("src/Comp{i}.tsx"), format!("export function Comp{i}() {{ return <Btn id=\"{i}\" />; }}\n")),
                2 => (format!("app/mod_{i}.py"), format!("def fn_{i}(x):\n    return helper(x)\n")),
                3 => (format!("app/Screen{i}.kt"), format!("@Composable\nfun Screen{i}() {{ Text(\"hi\") }}\n")),
                _ => (format!("lib/widget_{i}.dart"), format!("class W{i} extends StatelessWidget {{ void build() {{}} }}\n")),
            }
        })
        .collect();

    // Warm up rayon thread pool once or measure directly.
    let start = Instant::now();
    let results = analyze_files_parallel(&files, &cfg);
    let elapsed = start.elapsed();

    assert_eq!(results.len(), 50);
    assert!(results.iter().all(Result::is_ok));
    assert!(
        elapsed.as_millis() < 200,
        "expected 50 files in <200ms, took {}ms",
        elapsed.as_millis()
    );

    // Verify thread-local parser pool reuses parsers without duplicating per call.
    let _ = analyze_file(Path::new("src/a.ts"), "export const a = 1;", &cfg).expect("a");
    let count1 = anvil_ast::parser::cached_parser_count();
    let _ = analyze_file(Path::new("src/b.ts"), "export const b = 2;", &cfg).expect("b");
    let count2 = anvil_ast::parser::cached_parser_count();
    assert_eq!(count1, count2);
    assert!(count1 >= 1);
}
