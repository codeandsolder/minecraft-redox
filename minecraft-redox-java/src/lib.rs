//! Java source inventory frontend for Minecraft Redox.

use anyhow::{Context, Result};
use minecraft_redox_core::{SourceKind, SourceOrigin};
use serde::Serialize;
use tree_sitter::{Node, Parser, Tree};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaSymbolKind {
    Class,
    Interface,
    Enum,
    Record,
    Method,
    Constructor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaSymbol {
    pub kind: JavaSymbolKind,
    pub name: String,
    pub origin: SourceOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaInventory {
    pub path: String,
    pub has_parse_errors: bool,
    pub symbols: Vec<JavaSymbol>,
}

/// Parse Java source and inventory declaration-level symbols with source provenance.
///
/// # Errors
///
/// Returns an error if the Java grammar cannot be installed in the parser or if
/// tree-sitter fails to produce a syntax tree.
pub fn inventory_source(
    path: &str,
    artifact: &str,
    version: Option<&str>,
    source: &str,
) -> Result<JavaInventory> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .context("failed to load tree-sitter Java grammar")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter returned no Java syntax tree")?;
    Ok(inventory_tree(path, artifact, version, source, &tree))
}

fn inventory_tree(
    path: &str,
    artifact: &str,
    version: Option<&str>,
    source: &str,
    tree: &Tree,
) -> JavaInventory {
    let mut symbols = Vec::new();
    walk(
        tree.root_node(),
        source,
        path,
        artifact,
        version,
        &mut symbols,
    );
    JavaInventory {
        path: path.to_owned(),
        has_parse_errors: tree.root_node().has_error(),
        symbols,
    }
}

fn walk(
    node: Node<'_>,
    source: &str,
    path: &str,
    artifact: &str,
    version: Option<&str>,
    symbols: &mut Vec<JavaSymbol>,
) {
    if let Some(kind) = symbol_kind(node.kind())
        && let Some(name_node) = node.child_by_field_name("name")
        && let Ok(name) = name_node.utf8_text(source.as_bytes())
    {
        symbols.push(JavaSymbol {
            kind,
            name: name.to_owned(),
            origin: SourceOrigin {
                source_kind: SourceKind::Java,
                artifact: artifact.to_owned(),
                version: version.map(str::to_owned),
                path: path.to_owned(),
                symbol: Some(name.to_owned()),
                byte_start: Some(node.start_byte()),
                byte_end: Some(node.end_byte()),
            },
        });
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, path, artifact, version, symbols);
    }
}

fn symbol_kind(kind: &str) -> Option<JavaSymbolKind> {
    match kind {
        "class_declaration" => Some(JavaSymbolKind::Class),
        "interface_declaration" => Some(JavaSymbolKind::Interface),
        "enum_declaration" => Some(JavaSymbolKind::Enum),
        "record_declaration" => Some(JavaSymbolKind::Record),
        "method_declaration" => Some(JavaSymbolKind::Method),
        "constructor_declaration" => Some(JavaSymbolKind::Constructor),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{JavaSymbolKind, inventory_source};

    #[test]
    fn inventories_class_constructor_and_method() -> anyhow::Result<()> {
        let source = r"
            package demo;
            final class Boiler {
                Boiler(int pressure) {}
                boolean canRun() { return true; }
            }
        ";
        let inventory = inventory_source("Boiler.java", "demo", Some("1"), source)?;
        assert!(!inventory.has_parse_errors);
        assert_eq!(inventory.symbols.len(), 3);
        assert_eq!(inventory.symbols[0].kind, JavaSymbolKind::Class);
        assert_eq!(inventory.symbols[1].kind, JavaSymbolKind::Constructor);
        assert_eq!(inventory.symbols[2].kind, JavaSymbolKind::Method);
        Ok(())
    }
}
