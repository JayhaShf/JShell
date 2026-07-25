use std::path::Path;

pub fn detect_language(path: &str, first_line: Option<&str>) -> &'static str {
    let name = Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match name.as_str() {
        "dockerfile" => return "bash",
        "makefile" | "gnumakefile" => return "make",
        _ => {}
    }

    let extension = Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();

    let language = match extension {
        "json" | "jsonc" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "mdx" | "markdown" => "markdown",
        "sh" | "bash" | "zsh" => "bash",
        "py" => "python",
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "html" | "htm" => "html",
        "css" | "scss" => "css",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "go" => "go",
        "lua" => "lua",
        "sql" => "sql",
        "diff" | "patch" => "diff",
        _ => "text",
    };
    if language != "text" {
        return language;
    }

    let first_line = first_line.unwrap_or_default().to_ascii_lowercase();
    if first_line.starts_with("#!") {
        if first_line.contains("python") {
            return "python";
        }
        if first_line.contains("bash") || first_line.contains("/sh") || first_line.contains("zsh") {
            return "bash";
        }
    }
    "text"
}

#[cfg(test)]
mod tests {
    use super::detect_language;

    #[test]
    fn detects_special_names_extensions_and_shebangs() {
        assert_eq!(detect_language("/etc/Dockerfile", None), "bash");
        assert_eq!(detect_language("/srv/Makefile", None), "make");
        assert_eq!(detect_language("/etc/app/config.yaml", None), "yaml");
        assert_eq!(detect_language("/src/main.rs", None), "rust");
        assert_eq!(
            detect_language("/tmp/tool", Some("#!/usr/bin/env python3")),
            "python"
        );
        assert_eq!(detect_language("/tmp/notes.unknown", None), "text");
    }
}
