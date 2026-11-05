//! Language identity and the language server that owns it.

use std::path::Path;

/// One language the Build editor can hand to a language server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LanguageId {
    Astro,
    TypeScript,
    TypeScriptReact,
    JavaScript,
    JavaScriptReact,
    Json,
    Jsonc,
    Css,
    Scss,
    Less,
    Html,
    Svelte,
    Vue,
    Markdown,
}

/// How to start one language server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerDef {
    /// Stable server identity for logs and one-server-per-language reuse.
    pub id: &'static str,
    /// Executable names to look for, in order.
    pub binaries: &'static [&'static str],
    /// Fixed arguments passed before any client arguments.
    pub args: &'static [&'static str],
}

impl LanguageId {
    /// Resolve a language from a project-relative or absolute path.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path
            .extension()
            .and_then(std::ffi::OsStr::to_str)?
            .to_ascii_lowercase();
        Self::from_extension(&extension)
    }

    /// Resolve a language from a lowercase or mixed-case file extension.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "astro" => Some(Self::Astro),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::TypeScriptReact),
            "js" | "mjs" | "cjs" => Some(Self::JavaScript),
            "jsx" => Some(Self::JavaScriptReact),
            "json" => Some(Self::Json),
            "jsonc" => Some(Self::Jsonc),
            "css" => Some(Self::Css),
            "scss" => Some(Self::Scss),
            "less" => Some(Self::Less),
            "html" | "htm" => Some(Self::Html),
            "svelte" => Some(Self::Svelte),
            "vue" => Some(Self::Vue),
            "md" | "mdx" => Some(Self::Markdown),
            _ => None,
        }
    }

    /// The LSP `languageId` string sent in `textDocument/didOpen`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Astro => "astro",
            Self::TypeScript => "typescript",
            Self::TypeScriptReact => "typescriptreact",
            Self::JavaScript => "javascript",
            Self::JavaScriptReact => "javascriptreact",
            Self::Json => "json",
            Self::Jsonc => "jsonc",
            Self::Css => "css",
            Self::Scss => "scss",
            Self::Less => "less",
            Self::Html => "html",
            Self::Svelte => "svelte",
            Self::Vue => "vue",
            Self::Markdown => "markdown",
        }
    }

    /// The installed server that serves this language, if any.
    ///
    /// Markdown has no bundled server: Astro projects write prose, and the
    /// writer already provides their language tooling.
    pub fn server(self) -> Option<ServerDef> {
        match self {
            Self::Astro => Some(ServerDef {
                id: "astro",
                binaries: &["astro-ls"],
                args: &["--stdio"],
            }),
            Self::TypeScript | Self::TypeScriptReact | Self::JavaScript | Self::JavaScriptReact => {
                Some(ServerDef {
                    id: "typescript",
                    binaries: &["typescript-language-server"],
                    args: &["--stdio"],
                })
            }
            Self::Json | Self::Jsonc => Some(ServerDef {
                id: "json",
                binaries: &["vscode-json-language-server"],
                args: &["--stdio"],
            }),
            Self::Css | Self::Scss | Self::Less => Some(ServerDef {
                id: "css",
                binaries: &["vscode-css-language-server"],
                args: &["--stdio"],
            }),
            Self::Html => Some(ServerDef {
                id: "html",
                binaries: &["vscode-html-language-server"],
                args: &["--stdio"],
            }),
            Self::Svelte => Some(ServerDef {
                id: "svelte",
                binaries: &["svelteserver"],
                args: &["--stdio"],
            }),
            Self::Vue => Some(ServerDef {
                id: "vue",
                binaries: &["vue-language-server"],
                args: &["--stdio"],
            }),
            Self::Markdown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolves_the_astro_stack() {
        let cases = [
            ("src/pages/index.astro", LanguageId::Astro),
            ("src/lib/a.ts", LanguageId::TypeScript),
            ("src/lib/a.mts", LanguageId::TypeScript),
            ("src/components/Button.tsx", LanguageId::TypeScriptReact),
            ("src/scripts/a.mjs", LanguageId::JavaScript),
            ("src/components/Legacy.jsx", LanguageId::JavaScriptReact),
            ("package.json", LanguageId::Json),
            ("tsconfig.jsonc", LanguageId::Jsonc),
            ("src/styles/a.css", LanguageId::Css),
            ("src/styles/a.scss", LanguageId::Scss),
            ("src/styles/a.less", LanguageId::Less),
            ("src/index.html", LanguageId::Html),
            ("src/components/Widget.svelte", LanguageId::Svelte),
            ("src/components/Widget.vue", LanguageId::Vue),
            ("src/content/post.md", LanguageId::Markdown),
            ("src/content/post.mdx", LanguageId::Markdown),
        ];
        for (path, language) in cases {
            assert_eq!(
                LanguageId::from_path(&PathBuf::from(path)),
                Some(language),
                "{path}"
            );
        }
        assert_eq!(LanguageId::from_path(&PathBuf::from("README")), None);
        assert_eq!(LanguageId::from_path(&PathBuf::from("a.exe")), None);
    }

    #[test]
    fn maps_every_supported_language_to_its_server_or_none() {
        for language in [
            LanguageId::Astro,
            LanguageId::TypeScript,
            LanguageId::TypeScriptReact,
            LanguageId::JavaScript,
            LanguageId::JavaScriptReact,
            LanguageId::Json,
            LanguageId::Jsonc,
            LanguageId::Css,
            LanguageId::Scss,
            LanguageId::Less,
            LanguageId::Html,
            LanguageId::Svelte,
            LanguageId::Vue,
        ] {
            let server = language.server().unwrap_or_else(|| panic!("{language:?}"));
            assert!(!server.binaries.is_empty());
            assert_eq!(server.args, ["--stdio"]);
        }
        assert!(LanguageId::Markdown.server().is_none());
    }
}
