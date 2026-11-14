//! Capabilities implemented by the shared LSP client.

use serde_json::{Value, json};

/// The client capabilities CrabCraft implements.
pub fn client_capabilities() -> Value {
    json!({
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": { "documentChanges": true, "resourceOperations": ["create", "rename", "delete"] },
            "didChangeConfiguration": { "dynamicRegistration": true },
            "didChangeWatchedFiles": { "dynamicRegistration": true },
            "configuration": true,
            "workspaceFolders": true,
            "symbol": { "symbolKind": { "valueSet": (1..=26).collect::<Vec<i32>>() } },
            "semanticTokens": { "refreshSupport": true },
            "inlayHint": { "refreshSupport": true },
            "diagnostics": { "refreshSupport": true },
            "codeLens": { "refreshSupport": false }
        },
        "textDocument": {
            "synchronization": { "didSave": true, "willSave": false },
            "publishDiagnostics": { "relatedInformation": true, "tagSupport": { "valueSet": [1, 2] }, "versionSupport": true },
            "diagnostic": { "dynamicRegistration": true, "relatedDocumentSupport": false },
            "completion": {
                "completionItem": {
                    "snippetSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "deprecatedSupport": true,
                    "tagSupport": { "valueSet": [1] },
                    "insertReplaceSupport": false,
                    "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits", "textEdit"] },
                    "labelDetailsSupport": true
                },
                "completionItemKind": { "valueSet": (1..=25).collect::<Vec<i32>>() },
                "contextSupport": true,
                "completionList": { "itemDefaults": ["commitCharacters", "editRange", "insertTextFormat", "insertTextMode", "data"] }
            },
            "hover": { "contentFormat": ["markdown", "plaintext"] },
            "signatureHelp": {
                "signatureInformation": { "documentationFormat": ["markdown", "plaintext"], "parameterInformation": { "labelOffsetSupport": true }, "activeParameterSupport": true },
                "contextSupport": true
            },
            "definition": { "linkSupport": false },
            "typeDefinition": { "linkSupport": false },
            "implementation": { "linkSupport": false },
            "references": {},
            "documentHighlight": {},
            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true, "symbolKind": { "valueSet": (1..=26).collect::<Vec<i32>>() } },
            "codeAction": {
                "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"] } },
                "resolveSupport": { "properties": ["edit"] },
                "dataSupport": true,
                "isPreferredSupport": true
            },
            "formatting": {},
            "rangeFormatting": {},
            "onTypeFormatting": {},
            "rename": { "prepareSupport": true },
            "foldingRange": { "lineFoldingOnly": true },
            "semanticTokens": {
                "requests": { "range": true, "full": { "delta": false } },
                "tokenTypes": crate::semantic::CLIENT_TOKEN_TYPES,
                "tokenModifiers": crate::semantic::CLIENT_TOKEN_MODIFIERS,
                "formats": ["relative"],
                "overlappingTokenSupport": false,
                "multilineTokenSupport": false
            },
            "inlayHint": { "resolveSupport": { "properties": [] } }
        },
        "window": { "workDoneProgress": true, "showMessage": {} },
        "general": { "positionEncodings": ["utf-16"] }
    })
}
