//! Java source inventory and expression frontend for Minecraft Redox.

use std::{collections::BTreeMap, str::Utf8Error};

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum JavaExpression {
    Reference(String),
    Literal(String),
    Call {
        receiver: Option<Box<JavaExpression>>,
        method: String,
        arguments: Vec<JavaExpression>,
    },
    New {
        type_name: String,
        arguments: Vec<JavaExpression>,
    },
    Binary {
        left: Box<JavaExpression>,
        operator: String,
        right: Box<JavaExpression>,
    },
    Parenthesized(Box<JavaExpression>),
    Raw(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaCall {
    pub name: String,
    pub arguments: Vec<String>,
    pub argument_expressions: Vec<JavaExpression>,
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaCallChain {
    pub root: String,
    pub calls: Vec<JavaCall>,
    pub origin: SourceOrigin,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct JavaBindings {
    pub package: Option<String>,
    pub imports: BTreeMap<String, String>,
    pub static_imports: BTreeMap<String, String>,
    pub wildcard_imports: Vec<String>,
    pub static_wildcard_imports: Vec<String>,
}

impl JavaBindings {
    #[must_use]
    pub fn resolve_reference(&self, reference: &str) -> String {
        let reference = reference.trim();
        if !is_java_reference(reference) {
            return reference.to_owned();
        }
        if let Some(resolved) = self.static_imports.get(reference) {
            return resolved.clone();
        }
        let Some((head, tail)) = reference.split_once('.') else {
            return self
                .imports
                .get(reference)
                .cloned()
                .unwrap_or_else(|| reference.to_owned());
        };
        self.imports.get(head).map_or_else(
            || reference.to_owned(),
            |resolved| format!("{resolved}.{tail}"),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaParameter {
    pub type_name: String,
    pub name: String,
    pub varargs: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaReturn {
    pub expression: JavaExpression,
    pub origin: SourceOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaMethod {
    pub owner: String,
    pub name: String,
    pub return_type: String,
    pub parameters: Vec<JavaParameter>,
    pub is_static: bool,
    pub returns: Vec<JavaReturn>,
    pub origin: SourceOrigin,
}

impl JavaMethod {
    #[must_use]
    pub fn stable_id(&self) -> String {
        let parameters = self
            .parameters
            .iter()
            .map(|parameter| {
                if parameter.varargs {
                    format!("{}...", parameter.type_name)
                } else {
                    parameter.type_name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{}#{}({parameters})", self.owner, self.name)
    }

    #[must_use]
    pub fn accepts_arity(&self, arity: usize) -> bool {
        match self.parameters.last() {
            Some(parameter) if parameter.varargs => {
                arity >= self.parameters.len().saturating_sub(1)
            }
            _ => arity == self.parameters.len(),
        }
    }

    #[must_use]
    pub fn argument_bindings(
        &self,
        arguments: &[JavaExpression],
    ) -> Option<BTreeMap<String, JavaExpression>> {
        if self.parameters.iter().any(|parameter| parameter.varargs)
            || self.parameters.len() != arguments.len()
        {
            return None;
        }
        Some(
            self.parameters
                .iter()
                .zip(arguments)
                .map(|(parameter, argument)| (parameter.name.clone(), argument.clone()))
                .collect(),
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct JavaProjectIndex {
    methods_by_path: BTreeMap<String, Vec<JavaMethod>>,
}

impl JavaProjectIndex {
    pub fn add_analysis(&mut self, analysis: &JavaAnalysis) {
        for method in &analysis.methods {
            self.methods_by_path
                .entry(method.origin.path.clone())
                .or_default()
                .push(method.clone());
        }
    }

    #[must_use]
    pub fn merged(mut self, other: Self) -> Self {
        for (path, mut methods) in other.methods_by_path {
            self.methods_by_path
                .entry(path)
                .or_default()
                .append(&mut methods);
        }
        self
    }

    #[must_use]
    pub fn method_count(&self) -> usize {
        self.methods_by_path.values().map(Vec::len).sum()
    }

    #[must_use]
    pub fn resolve_local_method(
        &self,
        path: &str,
        name: &str,
        arity: usize,
    ) -> Option<&JavaMethod> {
        let mut matches = self
            .methods_by_path
            .get(path)?
            .iter()
            .filter(|method| method.name == name && method.accepts_arity(arity));
        let method = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(method)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaAnalysis {
    pub bindings: JavaBindings,
    pub inventory: JavaInventory,
    pub methods: Vec<JavaMethod>,
    pub call_chains: Vec<JavaCallChain>,
}

/// Parse Java source once and recover declaration inventory plus receiver call chains.
///
/// # Errors
///
/// Returns an error if the Java grammar cannot be installed in the parser or if
/// tree-sitter fails to produce a syntax tree.
pub fn analyze_source(
    path: &str,
    artifact: &str,
    version: Option<&str>,
    source: &str,
) -> Result<JavaAnalysis> {
    let tree = parse(source)?;
    let bindings = bindings_tree(tree.root_node(), source);
    let inventory = inventory_tree(path, artifact, version, source, &tree);
    let mut methods = Vec::new();
    collect_methods(
        tree.root_node(),
        source,
        path,
        artifact,
        version,
        &bindings,
        &mut methods,
    );
    let mut call_chains = Vec::new();
    collect_outer_call_chains(
        tree.root_node(),
        source,
        path,
        artifact,
        version,
        &mut call_chains,
    );
    Ok(JavaAnalysis {
        bindings,
        inventory,
        methods,
        call_chains,
    })
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
    Ok(analyze_source(path, artifact, version, source)?.inventory)
}

fn parse(source: &str) -> Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .context("failed to load tree-sitter Java grammar")?;
    parser
        .parse(source, None)
        .context("tree-sitter returned no Java syntax tree")
}

fn bindings_tree(root: Node<'_>, source: &str) -> JavaBindings {
    let mut bindings = JavaBindings::default();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        let Ok(text) = node_text(child, source) else {
            continue;
        };
        match child.kind() {
            "package_declaration" => {
                bindings.package = strip_statement(text, "package").map(str::to_owned);
            }
            "import_declaration" => add_import(&mut bindings, text),
            _ => {}
        }
    }
    bindings
}

fn add_import(bindings: &mut JavaBindings, text: &str) {
    let Some(body) = strip_statement(text, "import") else {
        return;
    };
    let (is_static, path) = body
        .strip_prefix("static ")
        .map_or((false, body), |path| (true, path.trim()));
    if let Some(prefix) = path.strip_suffix(".*") {
        if is_static {
            bindings.static_wildcard_imports.push(prefix.to_owned());
        } else {
            bindings.wildcard_imports.push(prefix.to_owned());
        }
        return;
    }
    let Some(simple) = path.rsplit('.').next() else {
        return;
    };
    if is_static {
        bindings
            .static_imports
            .insert(simple.to_owned(), path.to_owned());
    } else {
        bindings.imports.insert(simple.to_owned(), path.to_owned());
    }
}

fn strip_statement<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    text.trim()
        .strip_prefix(keyword)?
        .trim()
        .strip_suffix(';')
        .map(str::trim)
}

fn is_java_reference(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'.'))
}

fn inventory_tree(
    path: &str,
    artifact: &str,
    version: Option<&str>,
    source: &str,
    tree: &Tree,
) -> JavaInventory {
    let mut symbols = Vec::new();
    walk_symbols(
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

fn walk_symbols(
    node: Node<'_>,
    source: &str,
    path: &str,
    artifact: &str,
    version: Option<&str>,
    symbols: &mut Vec<JavaSymbol>,
) {
    if let Some(kind) = symbol_kind(node.kind())
        && let Some(name_node) = node.child_by_field_name("name")
        && let Ok(name) = node_text(name_node, source)
    {
        symbols.push(JavaSymbol {
            kind,
            name: name.to_owned(),
            origin: origin_for(node, Some(name), path, artifact, version),
        });
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_symbols(child, source, path, artifact, version, symbols);
    }
}

fn collect_methods(
    node: Node<'_>,
    source: &str,
    path: &str,
    artifact: &str,
    version: Option<&str>,
    bindings: &JavaBindings,
    methods: &mut Vec<JavaMethod>,
) {
    if node.kind() == "method_declaration"
        && let Some(method) = method_from_node(node, source, path, artifact, version, bindings)
    {
        methods.push(method);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_methods(child, source, path, artifact, version, bindings, methods);
    }
}

fn method_from_node(
    node: Node<'_>,
    source: &str,
    path: &str,
    artifact: &str,
    version: Option<&str>,
    bindings: &JavaBindings,
) -> Option<JavaMethod> {
    let name_node = node.child_by_field_name("name")?;
    let return_type_node = node.child_by_field_name("type")?;
    let parameters_node = node.child_by_field_name("parameters")?;
    let name = node_text(name_node, source).ok()?.to_owned();
    let return_type = bindings.resolve_reference(node_text(return_type_node, source).ok()?.trim());
    let owner = enclosing_type_name(node, source, bindings)?;
    let parameters = method_parameters(parameters_node, source, bindings);
    let is_static = method_is_static(node, source);
    let mut returns = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        collect_returns(body, source, path, artifact, version, &name, &mut returns);
    }
    Some(JavaMethod {
        owner,
        name: name.clone(),
        return_type,
        parameters,
        is_static,
        returns,
        origin: origin_for(node, Some(&name), path, artifact, version),
    })
}

fn method_parameters(
    parameters: Node<'_>,
    source: &str,
    bindings: &JavaBindings,
) -> Vec<JavaParameter> {
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .filter_map(|parameter| parameter_from_node(parameter, source, bindings))
        .collect()
}

fn parameter_from_node(
    parameter: Node<'_>,
    source: &str,
    bindings: &JavaBindings,
) -> Option<JavaParameter> {
    match parameter.kind() {
        "formal_parameter" => {
            let type_node = parameter.child_by_field_name("type")?;
            let name_node = parameter.child_by_field_name("name")?;
            Some(JavaParameter {
                type_name: bindings.resolve_reference(node_text(type_node, source).ok()?.trim()),
                name: node_text(name_node, source).ok()?.to_owned(),
                varargs: false,
            })
        }
        "spread_parameter" => spread_parameter(parameter, source, bindings),
        _ => None,
    }
}

fn spread_parameter(
    parameter: Node<'_>,
    source: &str,
    bindings: &JavaBindings,
) -> Option<JavaParameter> {
    let mut cursor = parameter.walk();
    let children: Vec<Node<'_>> = parameter.named_children(&mut cursor).collect();
    let variable = children
        .iter()
        .find(|child| child.kind() == "variable_declarator")?;
    let name_node = variable.child_by_field_name("name")?;
    let type_node = children.iter().find(|child| {
        !matches!(
            child.kind(),
            "modifiers" | "annotation" | "marker_annotation" | "variable_declarator"
        )
    })?;
    Some(JavaParameter {
        type_name: bindings.resolve_reference(node_text(*type_node, source).ok()?.trim()),
        name: node_text(name_node, source).ok()?.to_owned(),
        varargs: true,
    })
}

fn enclosing_type_name(node: Node<'_>, source: &str, bindings: &JavaBindings) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
        ) && let Some(name_node) = parent.child_by_field_name("name")
            && let Ok(name) = node_text(name_node, source)
        {
            return Some(
                bindings
                    .package
                    .as_ref()
                    .map_or_else(|| name.to_owned(), |package| format!("{package}.{name}")),
            );
        }
        current = parent.parent();
    }
    None
}

fn method_is_static(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == "modifiers")
        .and_then(|modifiers| node_text(modifiers, source).ok())
        .is_some_and(|modifiers| modifiers.split_whitespace().any(|token| token == "static"))
}

fn collect_returns(
    node: Node<'_>,
    source: &str,
    path: &str,
    artifact: &str,
    version: Option<&str>,
    symbol: &str,
    returns: &mut Vec<JavaReturn>,
) {
    if node.kind() == "return_statement" {
        if let Some(value) = node.named_child(0) {
            returns.push(JavaReturn {
                expression: expression(value, source),
                origin: origin_for(node, Some(symbol), path, artifact, version),
            });
        }
        return;
    }
    if matches!(
        node.kind(),
        "lambda_expression"
            | "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "method_declaration"
            | "constructor_declaration"
    ) {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_returns(child, source, path, artifact, version, symbol, returns);
    }
}

/// Replace syntactic reference expressions that match supplied bindings.
///
/// This is deliberately not name resolution. It is used after a call target has
/// already been linked to substitute formal parameters with actual arguments.
#[must_use]
pub fn substitute_references(
    expression: &JavaExpression,
    replacements: &BTreeMap<String, JavaExpression>,
) -> JavaExpression {
    match expression {
        JavaExpression::Reference(reference) => replacements
            .get(reference)
            .cloned()
            .unwrap_or_else(|| expression.clone()),
        JavaExpression::Call {
            receiver,
            method,
            arguments,
        } => JavaExpression::Call {
            receiver: receiver
                .as_ref()
                .map(|value| Box::new(substitute_references(value, replacements))),
            method: method.clone(),
            arguments: arguments
                .iter()
                .map(|value| substitute_references(value, replacements))
                .collect(),
        },
        JavaExpression::New {
            type_name,
            arguments,
        } => JavaExpression::New {
            type_name: type_name.clone(),
            arguments: arguments
                .iter()
                .map(|value| substitute_references(value, replacements))
                .collect(),
        },
        JavaExpression::Binary {
            left,
            operator,
            right,
        } => JavaExpression::Binary {
            left: Box::new(substitute_references(left, replacements)),
            operator: operator.clone(),
            right: Box::new(substitute_references(right, replacements)),
        },
        JavaExpression::Parenthesized(inner) => {
            JavaExpression::Parenthesized(Box::new(substitute_references(inner, replacements)))
        }
        JavaExpression::Literal(_) | JavaExpression::Raw(_) => expression.clone(),
    }
}

fn collect_outer_call_chains(
    node: Node<'_>,
    source: &str,
    path: &str,
    artifact: &str,
    version: Option<&str>,
    chains: &mut Vec<JavaCallChain>,
) {
    if node.kind() == "method_invocation"
        && !is_receiver_of_parent_invocation(node)
        && let Some((root, calls)) = call_chain(node, source)
    {
        chains.push(JavaCallChain {
            root,
            calls,
            origin: origin_for(
                node,
                enclosing_symbol(node, source).as_deref(),
                path,
                artifact,
                version,
            ),
        });
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_outer_call_chains(child, source, path, artifact, version, chains);
    }
}

fn is_receiver_of_parent_invocation(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "method_invocation" {
        return false;
    }
    parent
        .child_by_field_name("object")
        .is_some_and(|object| object.id() == node.id())
}

fn call_chain(node: Node<'_>, source: &str) -> Option<(String, Vec<JavaCall>)> {
    if node.kind() != "method_invocation" {
        return None;
    }
    let mut calls = Vec::new();
    let root = call_chain_inner(node, source, &mut calls)?;
    Some((root, calls))
}

fn call_chain_inner(node: Node<'_>, source: &str, calls: &mut Vec<JavaCall>) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source).ok()?.to_owned();
    let (arguments, argument_expressions) = node.child_by_field_name("arguments").map_or_else(
        || (Vec::new(), Vec::new()),
        |args| argument_data(args, source),
    );

    let root = match node.child_by_field_name("object") {
        Some(object) if object.kind() == "method_invocation" => {
            call_chain_inner(object, source, calls)?
        }
        Some(object) => node_text(object, source).ok()?.trim().to_owned(),
        None => String::from("<implicit>"),
    };

    calls.push(JavaCall {
        name,
        arguments,
        argument_expressions,
        byte_start: node.start_byte(),
        byte_end: node.end_byte(),
    });
    Some(root)
}

fn argument_data(arguments: Node<'_>, source: &str) -> (Vec<String>, Vec<JavaExpression>) {
    let mut cursor = arguments.walk();
    let nodes: Vec<Node<'_>> = arguments.named_children(&mut cursor).collect();
    let texts = nodes
        .iter()
        .filter_map(|argument| node_text(*argument, source).ok())
        .map(|text| text.trim().to_owned())
        .collect();
    let expressions = nodes
        .into_iter()
        .map(|argument| expression(argument, source))
        .collect();
    (texts, expressions)
}

fn expression(node: Node<'_>, source: &str) -> JavaExpression {
    match node.kind() {
        "identifier" | "field_access" | "scoped_identifier" => {
            JavaExpression::Reference(node_source(node, source))
        }
        "decimal_integer_literal"
        | "hex_integer_literal"
        | "octal_integer_literal"
        | "binary_integer_literal"
        | "decimal_floating_point_literal"
        | "hex_floating_point_literal"
        | "string_literal"
        | "character_literal"
        | "true"
        | "false"
        | "null_literal" => JavaExpression::Literal(node_source(node, source)),
        "method_invocation" => expression_call(node, source),
        "object_creation_expression" => expression_new(node, source),
        "binary_expression" => expression_binary(node, source),
        "parenthesized_expression" => node.named_child(0).map_or_else(
            || JavaExpression::Raw(node_source(node, source)),
            |child| JavaExpression::Parenthesized(Box::new(expression(child, source))),
        ),
        _ => JavaExpression::Raw(node_source(node, source)),
    }
}

fn expression_call(node: Node<'_>, source: &str) -> JavaExpression {
    let Some(name_node) = node.child_by_field_name("name") else {
        return JavaExpression::Raw(node_source(node, source));
    };
    let method = node_source(name_node, source);
    let receiver = node
        .child_by_field_name("object")
        .map(|object| Box::new(expression(object, source)));
    let arguments = node
        .child_by_field_name("arguments")
        .map_or_else(Vec::new, |args| argument_data(args, source).1);
    JavaExpression::Call {
        receiver,
        method,
        arguments,
    }
}

fn expression_new(node: Node<'_>, source: &str) -> JavaExpression {
    let Some(type_node) = node.child_by_field_name("type") else {
        return JavaExpression::Raw(node_source(node, source));
    };
    let type_name = node_source(type_node, source);
    let arguments = node
        .child_by_field_name("arguments")
        .map_or_else(Vec::new, |args| argument_data(args, source).1);
    JavaExpression::New {
        type_name,
        arguments,
    }
}

fn expression_binary(node: Node<'_>, source: &str) -> JavaExpression {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return JavaExpression::Raw(node_source(node, source));
    };
    let operator = source
        .get(left.end_byte()..right.start_byte())
        .map_or_else(String::new, |text| text.trim().to_owned());
    JavaExpression::Binary {
        left: Box::new(expression(left, source)),
        operator,
        right: Box::new(expression(right, source)),
    }
}

fn node_source(node: Node<'_>, source: &str) -> String {
    node_text(node, source).map_or_else(|_| String::new(), |text| text.trim().to_owned())
}

fn enclosing_symbol(node: Node<'_>, source: &str) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "method_declaration" | "constructor_declaration" | "class_declaration"
        ) && let Some(name) = parent.child_by_field_name("name")
            && let Ok(text) = node_text(name, source)
        {
            return Some(text.to_owned());
        }
        current = parent.parent();
    }
    None
}

fn origin_for(
    node: Node<'_>,
    symbol: Option<&str>,
    path: &str,
    artifact: &str,
    version: Option<&str>,
) -> SourceOrigin {
    SourceOrigin {
        source_kind: SourceKind::Java,
        artifact: artifact.to_owned(),
        version: version.map(str::to_owned),
        path: path.to_owned(),
        symbol: symbol.map(str::to_owned),
        byte_start: Some(node.start_byte()),
        byte_end: Some(node.end_byte()),
    }
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> std::result::Result<&'a str, Utf8Error> {
    node.utf8_text(source.as_bytes())
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
    use super::{JavaSymbolKind, analyze_source, inventory_source};

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

    #[test]
    fn recovers_receiver_chain_without_argument_calls_as_chain_members() -> anyhow::Result<()> {
        let source = r"
            class Recipes {
                void load() {
                    GTValues.RA.stdBuilder()
                        .itemInputs(ItemList.Cell_Water.get(1))
                        .duration(15 * SECONDS)
                        .addTo(compressorRecipes);
                }
            }
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        let chain = analysis
            .call_chains
            .iter()
            .find(|chain| chain.calls.iter().any(|call| call.name == "stdBuilder"))
            .ok_or_else(|| anyhow::anyhow!("recipe chain not recovered"))?;
        assert_eq!(chain.root, "GTValues.RA");
        let names: Vec<&str> = chain.calls.iter().map(|call| call.name.as_str()).collect();
        assert_eq!(names, ["stdBuilder", "itemInputs", "duration", "addTo"]);
        assert_eq!(chain.calls[1].arguments, ["ItemList.Cell_Water.get(1)"]);
        assert_eq!(chain.calls[2].argument_expressions.len(), 1);
        assert_eq!(chain.origin.symbol.as_deref(), Some("load"));
        Ok(())
    }

    #[test]
    fn recovers_constructor_rooted_builder_chain() -> anyhow::Result<()> {
        let source = r#"
            class MaterialsInit {
                void load() {
                    Materials foo = new MaterialBuilder().setName("Foo")
                        .setMeltingPoint(1000)
                        .constructMaterial();
                }
            }
        "#;
        let analysis = analyze_source("MaterialsInit.java", "gt5", None, source)?;
        let chain = analysis
            .call_chains
            .iter()
            .find(|chain| chain.root.starts_with("new MaterialBuilder"))
            .ok_or_else(|| anyhow::anyhow!("material builder chain not recovered"))?;
        assert_eq!(chain.calls[0].name, "setName");
        assert_eq!(chain.calls[2].name, "constructMaterial");
        Ok(())
    }

    #[test]
    fn indexes_method_signatures_and_resolves_unique_local_overloads() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import net.minecraft.item.ItemStack;
            class Factory {
                private static ItemStack get(String key, int amount) { return null; }
                private static ItemStack get(int amount) { return null; }
            }
        ";
        let analysis = analyze_source("Factory.java", "demo", None, source)?;
        assert_eq!(analysis.methods.len(), 2);
        assert_eq!(analysis.methods[0].owner, "demo.Factory");
        assert_eq!(
            analysis.methods[0].return_type,
            "net.minecraft.item.ItemStack"
        );
        assert!(analysis.methods[0].is_static);
        assert_eq!(analysis.methods[0].returns.len(), 1);
        assert_eq!(
            analysis.methods[0].stable_id(),
            "demo.Factory#get(String,int)"
        );

        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        assert_eq!(index.method_count(), 2);
        let method = index
            .resolve_local_method("Factory.java", "get", 2)
            .ok_or_else(|| anyhow::anyhow!("two-argument overload not resolved"))?;
        assert_eq!(method.parameters.len(), 2);
        assert_eq!(method.return_type, "net.minecraft.item.ItemStack");
        Ok(())
    }

    #[test]
    fn substitutes_method_parameters_in_return_expression() -> anyhow::Result<()> {
        let source = r"
            class Factory {
                static int twice(int amount) { return amount * 2; }
            }
        ";
        let analysis = analyze_source("Factory.java", "demo", None, source)?;
        let method = analysis
            .methods
            .first()
            .ok_or_else(|| anyhow::anyhow!("method missing"))?;
        let arguments = [super::JavaExpression::Literal(String::from("7"))];
        let bindings = method
            .argument_bindings(&arguments)
            .ok_or_else(|| anyhow::anyhow!("argument binding failed"))?;
        let substituted = super::substitute_references(&method.returns[0].expression, &bindings);
        assert!(matches!(substituted, super::JavaExpression::Binary { .. }));
        Ok(())
    }

    #[test]
    fn resolves_imported_types_and_static_members() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import gregtech.api.enums.GTValues;
            import static gregtech.api.recipe.RecipeMaps.compressorRecipes;
            class Recipes { void run() { GTValues.RA.stdBuilder().addTo(compressorRecipes); } }
        ";
        let analysis = analyze_source("Recipes.java", "gt5", None, source)?;
        assert_eq!(analysis.bindings.package.as_deref(), Some("demo"));
        assert_eq!(
            analysis.bindings.resolve_reference("GTValues.RA"),
            "gregtech.api.enums.GTValues.RA"
        );
        assert_eq!(
            analysis.bindings.resolve_reference("compressorRecipes"),
            "gregtech.api.recipe.RecipeMaps.compressorRecipes"
        );
        Ok(())
    }
}
