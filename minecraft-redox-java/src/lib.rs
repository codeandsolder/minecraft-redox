//! Java source inventory and expression frontend for Minecraft Redox.

use std::{
    collections::{BTreeMap, BTreeSet},
    str::Utf8Error,
};

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
    FieldAccess {
        receiver: Box<JavaExpression>,
        field: String,
    },
    Array {
        element_type: Option<String>,
        elements: Vec<JavaExpression>,
    },
    ArrayAccess {
        array: Box<JavaExpression>,
        index: Box<JavaExpression>,
    },
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
    Ternary {
        condition: Box<JavaExpression>,
        consequence: Box<JavaExpression>,
        alternative: Box<JavaExpression>,
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
pub struct JavaLocalVariable {
    pub type_name: String,
    pub name: String,
    pub declaration_byte: usize,
    pub scope_end_byte: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaField {
    pub owner: String,
    pub name: String,
    pub type_name: String,
    pub is_static: bool,
    pub origin: SourceOrigin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaValueKind {
    Local,
    Parameter,
    Field,
    StaticImportedField,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaValueResolution {
    pub kind: JavaValueKind,
    pub name: String,
    pub type_name: String,
    pub value_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaTypeDeclaration {
    pub owner: String,
    pub direct_supertypes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaMethod {
    pub owner: String,
    pub name: String,
    pub return_type: String,
    pub parameters: Vec<JavaParameter>,
    pub local_variables: Vec<JavaLocalVariable>,
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

#[derive(Clone, Copy)]
struct JavaCallSite<'a> {
    path: &'a str,
    byte_position: usize,
    arguments: &'a [JavaExpression],
    bindings: &'a JavaBindings,
}

#[derive(Clone, Copy)]
enum MethodFilter {
    Any,
    Static,
    Instance,
}

impl MethodFilter {
    fn matches(self, method: &JavaMethod) -> bool {
        match self {
            Self::Any => true,
            Self::Static => method.is_static,
            Self::Instance => !method.is_static,
        }
    }
}

#[derive(Clone, Copy)]
struct MethodQuery<'a> {
    name: &'a str,
    arity: usize,
    filter: MethodFilter,
}

#[derive(Default)]
struct HierarchyMethodState<'a> {
    visited: BTreeSet<String>,
    signatures: BTreeSet<String>,
    methods: Vec<&'a JavaMethod>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct JavaProjectIndex {
    methods: Vec<JavaMethod>,
    methods_by_path: BTreeMap<String, Vec<usize>>,
    methods_by_owner: BTreeMap<String, Vec<usize>>,
    fields: Vec<JavaField>,
    fields_by_owner: BTreeMap<String, Vec<usize>>,
    supertypes_by_owner: BTreeMap<String, Vec<String>>,
}

impl JavaProjectIndex {
    pub fn add_analysis(&mut self, analysis: &JavaAnalysis) {
        for method in &analysis.methods {
            let index = self.methods.len();
            self.methods.push(method.clone());
            self.methods_by_path
                .entry(method.origin.path.clone())
                .or_default()
                .push(index);
            self.methods_by_owner
                .entry(method.owner.clone())
                .or_default()
                .push(index);
        }
        for field in &analysis.fields {
            let index = self.fields.len();
            self.fields.push(field.clone());
            self.fields_by_owner
                .entry(field.owner.clone())
                .or_default()
                .push(index);
        }
        for declaration in &analysis.types {
            self.supertypes_by_owner
                .entry(declaration.owner.clone())
                .or_default()
                .extend(declaration.direct_supertypes.iter().cloned());
        }
    }

    #[must_use]
    pub fn merged(mut self, other: Self) -> Self {
        let method_offset = self.methods.len();
        self.methods.extend(other.methods);
        merge_index_map(
            &mut self.methods_by_path,
            other.methods_by_path,
            method_offset,
        );
        merge_index_map(
            &mut self.methods_by_owner,
            other.methods_by_owner,
            method_offset,
        );

        let field_offset = self.fields.len();
        self.fields.extend(other.fields);
        merge_index_map(
            &mut self.fields_by_owner,
            other.fields_by_owner,
            field_offset,
        );
        for (owner, supertypes) in other.supertypes_by_owner {
            self.supertypes_by_owner
                .entry(owner)
                .or_default()
                .extend(supertypes);
        }
        self
    }

    #[must_use]
    pub fn method_count(&self) -> usize {
        self.methods.len()
    }

    #[must_use]
    pub fn resolve_local_method(
        &self,
        path: &str,
        name: &str,
        arity: usize,
    ) -> Option<&JavaMethod> {
        unique_method(
            self.methods_by_path
                .get(path)?
                .iter()
                .map(|index| &self.methods[*index])
                .filter(|method| method.name == name && method.accepts_arity(arity)),
        )
    }

    #[must_use]
    pub fn resolve_method(&self, owner: &str, name: &str, arity: usize) -> Option<&JavaMethod> {
        self.resolve_method_filtered(owner, name, arity, MethodFilter::Any)
    }

    #[must_use]
    pub fn resolve_static_method(
        &self,
        owner: &str,
        name: &str,
        arity: usize,
    ) -> Option<&JavaMethod> {
        self.resolve_method_filtered(owner, name, arity, MethodFilter::Static)
    }

    #[must_use]
    pub fn resolve_instance_method(
        &self,
        owner: &str,
        name: &str,
        arity: usize,
    ) -> Option<&JavaMethod> {
        self.resolve_method_filtered(owner, name, arity, MethodFilter::Instance)
    }

    fn resolve_method_filtered(
        &self,
        owner: &str,
        name: &str,
        arity: usize,
        filter: MethodFilter,
    ) -> Option<&JavaMethod> {
        let query = MethodQuery {
            name,
            arity,
            filter,
        };
        let methods = self.methods_in_hierarchy(owner, query);
        unique_method(methods.into_iter())
    }

    fn methods_in_hierarchy<'a>(
        &'a self,
        owner: &str,
        query: MethodQuery<'_>,
    ) -> Vec<&'a JavaMethod> {
        let owner = self.normalize_owner_name(owner, None);
        let mut state = HierarchyMethodState::default();
        self.collect_hierarchy_methods(&owner, query, &mut state);
        state.methods
    }

    fn collect_hierarchy_methods<'a>(
        &'a self,
        owner: &str,
        query: MethodQuery<'_>,
        state: &mut HierarchyMethodState<'a>,
    ) {
        if !state.visited.insert(owner.to_owned()) {
            return;
        }
        if let Some(indexes) = self.methods_by_owner.get(owner) {
            for method in indexes
                .iter()
                .map(|index| &self.methods[*index])
                .filter(|method| {
                    method.name == query.name
                        && method.accepts_arity(query.arity)
                        && query.filter.matches(method)
                })
            {
                let signature = method_dispatch_signature(method);
                if state.signatures.insert(signature) {
                    state.methods.push(method);
                }
            }
        }
        if let Some(supertypes) = self.supertypes_by_owner.get(owner) {
            for supertype in supertypes {
                let normalized = self.normalize_owner_name(
                    supertype,
                    owner.rsplit_once('.').map(|(package, _)| package),
                );
                self.collect_hierarchy_methods(&normalized, query, state);
            }
        }
    }

    fn resolve_local_method_typed(
        &self,
        name: &str,
        site: JavaCallSite<'_>,
    ) -> Option<&JavaMethod> {
        let methods = self
            .methods_by_path
            .get(site.path)?
            .iter()
            .map(|index| &self.methods[*index]);
        self.select_typed_method(
            methods
                .filter(|method| method.name == name && method.accepts_arity(site.arguments.len())),
            site,
        )
    }

    fn resolve_method_filtered_typed(
        &self,
        owner: &str,
        name: &str,
        site: JavaCallSite<'_>,
        filter: MethodFilter,
    ) -> Option<&JavaMethod> {
        let query = MethodQuery {
            name,
            arity: site.arguments.len(),
            filter,
        };
        let methods = self.methods_in_hierarchy(owner, query);
        self.select_typed_method(methods.into_iter(), site)
    }

    fn select_typed_method<'a>(
        &'a self,
        methods: impl Iterator<Item = &'a JavaMethod>,
        site: JavaCallSite<'_>,
    ) -> Option<&'a JavaMethod> {
        let mut best: Option<(u32, &'a JavaMethod)> = None;
        let mut tied = false;
        for method in methods {
            let score = self.method_type_score(method, site);
            match best {
                None => {
                    best = Some((score, method));
                    tied = false;
                }
                Some((best_score, _)) if score > best_score => {
                    best = Some((score, method));
                    tied = false;
                }
                Some((best_score, _)) if score == best_score => tied = true,
                Some(_) => {}
            }
        }
        (!tied).then(|| best.map(|(_, method)| method)).flatten()
    }

    fn method_type_score(&self, method: &JavaMethod, site: JavaCallSite<'_>) -> u32 {
        let varargs = method
            .parameters
            .last()
            .is_some_and(|parameter| parameter.varargs);
        let fixed_count = method.parameters.len().saturating_sub(usize::from(varargs));
        let mut score = u32::from(!varargs);
        for (index, argument) in site.arguments.iter().enumerate() {
            let Some(actual) =
                self.expression_type_at(site.path, site.byte_position, argument, site.bindings)
            else {
                continue;
            };
            let Some(parameter) = method.parameters.get(index.min(fixed_count)) else {
                continue;
            };
            let package = method.owner.rsplit_once('.').map(|(package, _)| package);
            let expected = self.normalize_type_name(&parameter.type_name, package);
            score += java_type_match_score(&actual, &expected);
        }
        score
    }

    #[must_use]
    pub fn resolve_field(
        &self,
        owner: &str,
        name: &str,
        require_static: Option<bool>,
    ) -> Option<&JavaField> {
        let owner = self.normalize_owner_name(owner, None);
        let mut fields = self
            .fields_by_owner
            .get(&owner)?
            .iter()
            .map(|index| &self.fields[*index])
            .filter(|field| {
                field.name == name
                    && require_static.is_none_or(|required| field.is_static == required)
            });
        let field = fields.next()?;
        if fields.next().is_some() {
            return None;
        }
        Some(field)
    }

    #[must_use]
    pub fn resolve_call_at<'a>(
        &'a self,
        path: &str,
        byte_position: usize,
        expression: &JavaExpression,
        bindings: &JavaBindings,
    ) -> Option<(&'a JavaMethod, Option<String>)> {
        let JavaExpression::Call {
            receiver,
            method,
            arguments,
        } = expression
        else {
            return None;
        };
        let site = JavaCallSite {
            path,
            byte_position,
            arguments,
            bindings,
        };
        match receiver.as_deref() {
            None => self.resolve_implicit_call_at(method, site),
            Some(JavaExpression::Reference(reference)) => {
                self.resolve_reference_call_at(reference, method, site)
            }
            Some(receiver) => self.resolve_expression_call_at(receiver, method, site),
        }
    }

    fn resolve_implicit_call_at<'a>(
        &'a self,
        method: &str,
        site: JavaCallSite<'_>,
    ) -> Option<(&'a JavaMethod, Option<String>)> {
        if let Some(local) = self.resolve_local_method(site.path, method, site.arguments.len()) {
            return Some((local, None));
        }
        if let Some(local) = self.resolve_local_method_typed(method, site) {
            return Some((local, None));
        }
        if let Some(imported) =
            self.resolve_static_import(site.bindings, method, site.arguments.len())
        {
            return Some((imported, None));
        }
        self.resolve_static_import_typed(method, site)
            .map(|imported| (imported, None))
    }

    fn resolve_reference_call_at<'a>(
        &'a self,
        reference: &str,
        method: &str,
        site: JavaCallSite<'_>,
    ) -> Option<(&'a JavaMethod, Option<String>)> {
        if let Some(receiver_type) =
            self.reference_type_at(site.path, site.byte_position, reference)
        {
            let receiver_type = self.normalize_type_name_at(
                site.path,
                site.byte_position,
                receiver_type,
                site.bindings,
            );
            return self
                .resolve_instance_call(&receiver_type, method, site)
                .map(|resolved| (resolved, Some(receiver_type)));
        }
        if let Some(receiver_type) = self.imported_static_field_type(site.bindings, reference) {
            return self
                .resolve_instance_call(&receiver_type, method, site)
                .map(|resolved| (resolved, Some(receiver_type)));
        }
        let owner = self.class_owner_for_reference_at(
            site.path,
            site.byte_position,
            site.bindings,
            reference,
        )?;
        self.resolve_static_call(&owner, method, site)
            .map(|resolved| (resolved, Some(owner)))
    }

    fn resolve_expression_call_at<'a>(
        &'a self,
        receiver: &JavaExpression,
        method: &str,
        site: JavaCallSite<'_>,
    ) -> Option<(&'a JavaMethod, Option<String>)> {
        let receiver_type =
            self.expression_type_at(site.path, site.byte_position, receiver, site.bindings)?;
        self.resolve_instance_call(&receiver_type, method, site)
            .map(|resolved| (resolved, Some(receiver_type)))
    }

    fn resolve_instance_call<'a>(
        &'a self,
        owner: &str,
        method: &str,
        site: JavaCallSite<'_>,
    ) -> Option<&'a JavaMethod> {
        self.resolve_instance_method(owner, method, site.arguments.len())
            .or_else(|| {
                self.resolve_method_filtered_typed(owner, method, site, MethodFilter::Instance)
            })
    }

    fn resolve_static_call<'a>(
        &'a self,
        owner: &str,
        method: &str,
        site: JavaCallSite<'_>,
    ) -> Option<&'a JavaMethod> {
        self.resolve_static_method(owner, method, site.arguments.len())
            .or_else(|| {
                self.resolve_method_filtered_typed(owner, method, site, MethodFilter::Static)
            })
    }

    #[must_use]
    pub fn expression_type_at(
        &self,
        path: &str,
        byte_position: usize,
        expression: &JavaExpression,
        bindings: &JavaBindings,
    ) -> Option<String> {
        match expression {
            JavaExpression::Reference(reference) => self
                .reference_type_at(path, byte_position, reference)
                .map(|type_name| {
                    self.normalize_type_name_at(path, byte_position, type_name, bindings)
                })
                .or_else(|| self.imported_static_field_type(bindings, reference)),
            JavaExpression::FieldAccess { receiver, field } => {
                let static_owner = qualified_reference_name(receiver).and_then(|reference| {
                    let root = reference.split('.').next()?;
                    if self.reference_type_at(path, byte_position, root).is_some() {
                        return None;
                    }
                    self.class_owner_for_reference_at(path, byte_position, bindings, &reference)
                });
                let (owner, is_static) = static_owner.map_or_else(
                    || {
                        self.expression_type_at(path, byte_position, receiver, bindings)
                            .map(|owner| (owner, false))
                    },
                    |owner| Some((owner, true)),
                )?;
                let resolved = self.resolve_field(&owner, field, Some(is_static))?;
                Some(self.normalize_type_name(
                    &resolved.type_name,
                    resolved.owner.rsplit_once('.').map(|(package, _)| package),
                ))
            }
            JavaExpression::Array { element_type, .. } => element_type
                .as_ref()
                .map(|type_name| format!("{}[]", resolve_type_reference(bindings, type_name))),
            JavaExpression::ArrayAccess { array, .. } => {
                let array_type = self.expression_type_at(path, byte_position, array, bindings)?;
                let element_type = array_element_type(&array_type)?;
                Some(self.normalize_owner_name(element_type, bindings.package.as_deref()))
            }
            JavaExpression::Call {
                receiver,
                method,
                arguments,
            } => self
                .standard_library_call_type(
                    path,
                    byte_position,
                    receiver.as_deref(),
                    method,
                    arguments,
                    bindings,
                )
                .or_else(|| {
                    self.resolve_call_at(path, byte_position, expression, bindings)
                        .map(|(resolved, _)| {
                            self.normalize_type_name(
                                &resolved.return_type,
                                resolved.owner.rsplit_once('.').map(|(package, _)| package),
                            )
                        })
                }),
            JavaExpression::New { type_name, .. } => Some(self.normalize_type_name(
                &resolve_type_reference(bindings, type_name),
                bindings.package.as_deref(),
            )),
            JavaExpression::Ternary {
                consequence,
                alternative,
                ..
            } => self.ternary_expression_type_at(
                path,
                byte_position,
                consequence,
                alternative,
                bindings,
            ),
            JavaExpression::Parenthesized(inner) => {
                self.expression_type_at(path, byte_position, inner, bindings)
            }
            JavaExpression::Literal(value) => Some(literal_java_type(value).to_owned()),
            JavaExpression::Binary { .. } | JavaExpression::Raw(_) => None,
        }
    }

    fn ternary_expression_type_at(
        &self,
        path: &str,
        byte_position: usize,
        consequence: &JavaExpression,
        alternative: &JavaExpression,
        bindings: &JavaBindings,
    ) -> Option<String> {
        let consequence_type =
            self.expression_type_at(path, byte_position, consequence, bindings)?;
        let alternative_type =
            self.expression_type_at(path, byte_position, alternative, bindings)?;
        if consequence_type == alternative_type {
            return Some(consequence_type);
        }
        if consequence_type == "null" && !is_java_primitive(&alternative_type) {
            return Some(alternative_type);
        }
        if alternative_type == "null" && !is_java_primitive(&consequence_type) {
            return Some(consequence_type);
        }
        None
    }

    fn standard_library_call_type(
        &self,
        path: &str,
        byte_position: usize,
        receiver: Option<&JavaExpression>,
        method: &str,
        arguments: &[JavaExpression],
        bindings: &JavaBindings,
    ) -> Option<String> {
        let receiver = receiver?;
        if method == "requireNonNull"
            && matches!(arguments.len(), 1 | 2)
            && resolved_reference_name(receiver, bindings).as_deref() == Some("java.util.Objects")
        {
            return self.expression_type_at(path, byte_position, arguments.first()?, bindings);
        }
        let receiver_type = self.expression_type_at(path, byte_position, receiver, bindings)?;
        let raw_receiver = erase_java_type_syntax(&receiver_type);
        if method == "get" && arguments.is_empty() && is_optional_type(&raw_receiver) {
            let value_type = generic_type_argument(&receiver_type, 0)?;
            return Some(self.normalize_type_name(value_type, bindings.package.as_deref()));
        }
        if method == "get" && arguments.len() == 1 {
            if is_java_list_type(&raw_receiver) {
                let value_type = generic_type_argument(&receiver_type, 0)?;
                return Some(self.normalize_type_name(value_type, bindings.package.as_deref()));
            }
            if is_java_map_type(&raw_receiver) {
                let value_type = generic_type_argument(&receiver_type, 1)?;
                return Some(self.normalize_type_name(value_type, bindings.package.as_deref()));
            }
        }
        if method != "toArray" || arguments.len() != 1 || !is_java_collection_type(&raw_receiver) {
            return None;
        }
        let argument_type =
            self.expression_type_at(path, byte_position, arguments.first()?, bindings)?;
        argument_type.ends_with("[]").then_some(argument_type)
    }

    fn resolve_static_import(
        &self,
        bindings: &JavaBindings,
        method_name: &str,
        arity: usize,
    ) -> Option<&JavaMethod> {
        if let Some(path) = bindings.static_imports.get(method_name)
            && let Some((owner, imported_name)) = path.rsplit_once('.')
            && imported_name == method_name
        {
            return self.resolve_static_method(owner, method_name, arity);
        }

        unique_method(
            bindings
                .static_wildcard_imports
                .iter()
                .filter_map(|owner| self.resolve_static_method(owner, method_name, arity)),
        )
    }

    fn resolve_static_import_typed(
        &self,
        method_name: &str,
        site: JavaCallSite<'_>,
    ) -> Option<&JavaMethod> {
        if let Some(import_path) = site.bindings.static_imports.get(method_name)
            && let Some((owner, imported_name)) = import_path.rsplit_once('.')
            && imported_name == method_name
        {
            return self.resolve_method_filtered_typed(
                owner,
                method_name,
                site,
                MethodFilter::Static,
            );
        }
        let mut methods = site
            .bindings
            .static_wildcard_imports
            .iter()
            .filter_map(|owner| {
                self.resolve_method_filtered_typed(owner, method_name, site, MethodFilter::Static)
            });
        let method = methods.next()?;
        methods.next().is_none().then_some(method)
    }

    fn imported_static_field_type(
        &self,
        bindings: &JavaBindings,
        reference: &str,
    ) -> Option<String> {
        let field = self.imported_static_field(bindings, reference)?;
        Some(self.normalize_type_name(
            &field.type_name,
            field.owner.rsplit_once('.').map(|(package, _)| package),
        ))
    }

    fn imported_static_field<'a>(
        &'a self,
        bindings: &JavaBindings,
        reference: &str,
    ) -> Option<&'a JavaField> {
        if let Some(path) = bindings.static_imports.get(reference)
            && let Some((owner, field_name)) = path.rsplit_once('.')
        {
            return self.resolve_field(owner, field_name, Some(true));
        }

        let mut fields = bindings
            .static_wildcard_imports
            .iter()
            .filter_map(|owner| self.resolve_field(owner, reference, Some(true)));
        let field = fields.next()?;
        if fields.next().is_some() {
            return None;
        }
        Some(field)
    }

    fn class_owner_for_reference_at(
        &self,
        path: &str,
        byte_position: usize,
        bindings: &JavaBindings,
        reference: &str,
    ) -> Option<String> {
        if self.has_owner(reference) {
            return Some(reference.to_owned());
        }
        if let Some(method) = self.enclosing_method(path, byte_position) {
            let mut owner = Some(method.owner.as_str());
            while let Some(current) = owner {
                if let Some(candidate) = self.member_type_in_hierarchy(current, reference) {
                    return Some(candidate);
                }
                owner = current.rsplit_once('.').map(|(parent, _)| parent);
            }
        }
        self.class_owner_for_reference(bindings, reference)
    }

    fn member_type_in_hierarchy(&self, owner: &str, reference: &str) -> Option<String> {
        let mut visited = BTreeSet::new();
        self.member_type_in_hierarchy_inner(owner, reference, &mut visited)
    }

    fn member_type_in_hierarchy_inner(
        &self,
        owner: &str,
        reference: &str,
        visited: &mut BTreeSet<String>,
    ) -> Option<String> {
        if !visited.insert(owner.to_owned()) {
            return None;
        }
        let candidate = format!("{owner}.{reference}");
        if self.has_owner(&candidate) {
            return Some(candidate);
        }
        for supertype in self.supertypes_by_owner.get(owner)? {
            let normalized = self.normalize_owner_name(
                supertype,
                owner.rsplit_once('.').map(|(package, _)| package),
            );
            if let Some(candidate) =
                self.member_type_in_hierarchy_inner(&normalized, reference, visited)
            {
                return Some(candidate);
            }
        }
        None
    }

    fn class_owner_for_reference(
        &self,
        bindings: &JavaBindings,
        reference: &str,
    ) -> Option<String> {
        let resolved = bindings.resolve_reference(reference);
        let normalized = self.normalize_owner_name(&resolved, bindings.package.as_deref());
        self.has_owner(&normalized).then_some(normalized)
    }

    fn normalize_type_name_at(
        &self,
        path: &str,
        byte_position: usize,
        type_name: &str,
        bindings: &JavaBindings,
    ) -> String {
        let mut base = type_name.trim();
        let mut dimensions = 0usize;
        while let Some(stripped) = base.strip_suffix("[]") {
            dimensions += 1;
            base = stripped.trim_end();
        }
        let (raw_type, generic_suffix) = base
            .find('<')
            .map_or((base, ""), |index| (&base[..index], &base[index..]));
        let resolved_reference = bindings.resolve_reference(raw_type.trim());
        let mut normalized = self
            .class_owner_for_reference_at(path, byte_position, bindings, &resolved_reference)
            .unwrap_or_else(|| {
                self.normalize_owner_name(&resolved_reference, bindings.package.as_deref())
            });
        normalized.push_str(generic_suffix);
        for _ in 0..dimensions {
            normalized.push_str("[]");
        }
        normalized
    }

    fn normalize_type_name(&self, type_name: &str, package: Option<&str>) -> String {
        let mut base = type_name.trim();
        let mut dimensions = 0usize;
        while let Some(stripped) = base.strip_suffix("[]") {
            dimensions += 1;
            base = stripped.trim_end();
        }
        let (raw_type, generic_suffix) = base
            .find('<')
            .map_or((base, ""), |index| (&base[..index], &base[index..]));
        let mut normalized = self.normalize_owner_name(raw_type.trim(), package);
        normalized.push_str(generic_suffix);
        for _ in 0..dimensions {
            normalized.push_str("[]");
        }
        normalized
    }

    fn normalize_owner_name(&self, type_name: &str, package: Option<&str>) -> String {
        let type_name = erase_java_type_syntax(type_name);
        if self.has_owner(&type_name) {
            return type_name;
        }
        if let Some(context) = package {
            let mut prefix = Some(context);
            while let Some(current) = prefix {
                let candidate = format!("{current}.{type_name}");
                if self.has_owner(&candidate) {
                    return candidate;
                }
                prefix = current.rsplit_once('.').map(|(parent, _)| parent);
            }
        }
        type_name
    }

    fn has_owner(&self, owner: &str) -> bool {
        self.methods_by_owner.contains_key(owner)
            || self.fields_by_owner.contains_key(owner)
            || self.supertypes_by_owner.contains_key(owner)
    }

    #[must_use]
    pub fn enclosing_method(&self, path: &str, byte_position: usize) -> Option<&JavaMethod> {
        self.methods_by_path
            .get(path)?
            .iter()
            .map(|index| &self.methods[*index])
            .filter(|method| {
                method
                    .origin
                    .byte_start
                    .is_some_and(|start| start <= byte_position)
                    && method
                        .origin
                        .byte_end
                        .is_some_and(|end| byte_position < end)
            })
            .min_by_key(|method| {
                method
                    .origin
                    .byte_end
                    .unwrap_or(usize::MAX)
                    .saturating_sub(method.origin.byte_start.unwrap_or(0))
            })
    }

    #[must_use]
    pub fn resolve_value_at(
        &self,
        path: &str,
        byte_position: usize,
        reference_name: &str,
        bindings: &JavaBindings,
    ) -> Option<JavaValueResolution> {
        let method = self.enclosing_method(path, byte_position)?;
        if let Some(local) = method
            .local_variables
            .iter()
            .filter(|local| {
                local.name == reference_name
                    && local.declaration_byte <= byte_position
                    && byte_position < local.scope_end_byte
            })
            .max_by_key(|local| local.declaration_byte)
        {
            return Some(JavaValueResolution {
                kind: JavaValueKind::Local,
                name: local.name.clone(),
                type_name: self.normalize_type_name(
                    &local.type_name,
                    method.owner.rsplit_once('.').map(|(package, _)| package),
                ),
                value_id: format!(
                    "{}::local:{}@{}",
                    method.stable_id(),
                    local.name,
                    local.declaration_byte
                ),
            });
        }
        if let Some(parameter) = method
            .parameters
            .iter()
            .find(|parameter| parameter.name == reference_name)
        {
            return Some(JavaValueResolution {
                kind: JavaValueKind::Parameter,
                name: parameter.name.clone(),
                type_name: self.normalize_type_name(
                    &parameter.type_name,
                    method.owner.rsplit_once('.').map(|(package, _)| package),
                ),
                value_id: format!("{}::param:{}", method.stable_id(), parameter.name),
            });
        }
        if let Some(field) = self.resolve_field(&method.owner, reference_name, None) {
            return Some(JavaValueResolution {
                kind: JavaValueKind::Field,
                name: field.name.clone(),
                type_name: self.normalize_type_name(
                    &field.type_name,
                    field.owner.rsplit_once('.').map(|(package, _)| package),
                ),
                value_id: format!("{}::field:{}", field.owner, field.name),
            });
        }
        let field = self.imported_static_field(bindings, reference_name)?;
        Some(JavaValueResolution {
            kind: JavaValueKind::StaticImportedField,
            name: field.name.clone(),
            type_name: self.normalize_type_name(
                &field.type_name,
                field.owner.rsplit_once('.').map(|(package, _)| package),
            ),
            value_id: format!("{}::field:{}", field.owner, field.name),
        })
    }

    #[must_use]
    pub fn reference_type_at(
        &self,
        path: &str,
        byte_position: usize,
        reference_name: &str,
    ) -> Option<&str> {
        let method = self.enclosing_method(path, byte_position)?;
        if let Some(local) = method
            .local_variables
            .iter()
            .filter(|local| {
                local.name == reference_name
                    && local.declaration_byte <= byte_position
                    && byte_position < local.scope_end_byte
            })
            .max_by_key(|local| local.declaration_byte)
        {
            return Some(local.type_name.as_str());
        }
        if let Some(parameter) = method
            .parameters
            .iter()
            .find(|parameter| parameter.name == reference_name)
        {
            return Some(parameter.type_name.as_str());
        }
        self.resolve_field(&method.owner, reference_name, None)
            .map(|field| field.type_name.as_str())
    }

    #[must_use]
    pub fn parameter_type_at(
        &self,
        path: &str,
        byte_position: usize,
        parameter_name: &str,
    ) -> Option<&str> {
        let method = self.enclosing_method(path, byte_position)?;
        method
            .parameters
            .iter()
            .find(|parameter| parameter.name == parameter_name)
            .map(|parameter| parameter.type_name.as_str())
    }
}

fn resolve_type_reference(bindings: &JavaBindings, type_name: &str) -> String {
    let mut base = type_name.trim();
    let mut dimensions = 0usize;
    while let Some(stripped) = base.strip_suffix("[]") {
        dimensions += 1;
        base = stripped.trim_end();
    }
    let (raw_type, generic_suffix) = base
        .find('<')
        .map_or((base, ""), |index| (&base[..index], &base[index..]));
    let mut resolved = bindings.resolve_reference(raw_type.trim());
    resolved.push_str(generic_suffix);
    for _ in 0..dimensions {
        resolved.push_str("[]");
    }
    resolved
}

fn is_java_collection_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "java.util.Collection"
            | "java.util.List"
            | "java.util.ArrayList"
            | "java.util.LinkedList"
            | "java.util.Vector"
            | "java.util.Stack"
            | "java.util.Set"
            | "java.util.HashSet"
            | "java.util.LinkedHashSet"
            | "java.util.SortedSet"
            | "java.util.TreeSet"
            | "java.util.Queue"
            | "java.util.Deque"
            | "java.util.ArrayDeque"
    )
}

fn is_java_list_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "java.util.List"
            | "java.util.ArrayList"
            | "java.util.LinkedList"
            | "java.util.Vector"
            | "java.util.Stack"
    )
}

fn is_java_map_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "java.util.Map"
            | "java.util.HashMap"
            | "java.util.LinkedHashMap"
            | "java.util.SortedMap"
            | "java.util.NavigableMap"
            | "java.util.TreeMap"
            | "java.util.concurrent.ConcurrentMap"
            | "java.util.concurrent.ConcurrentHashMap"
    )
}

fn is_optional_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "java.util.Optional" | "com.google.common.base.Optional"
    )
}

fn resolved_reference_name(expression: &JavaExpression, bindings: &JavaBindings) -> Option<String> {
    qualified_reference_name(expression).map(|reference| bindings.resolve_reference(&reference))
}

fn generic_type_argument(type_name: &str, target_index: usize) -> Option<&str> {
    let mut argument_start = type_name.find('<')? + 1;
    let bytes = type_name.as_bytes();
    let mut depth = 1usize;
    let mut argument_index = 0usize;
    for (index, byte) in bytes.iter().copied().enumerate().skip(argument_start) {
        match byte {
            b'<' => depth += 1,
            b',' if depth == 1 => {
                if argument_index == target_index {
                    return valid_generic_argument(type_name.get(argument_start..index)?);
                }
                argument_index += 1;
                argument_start = index + 1;
            }
            b'>' if depth == 1 => {
                return (argument_index == target_index)
                    .then(|| valid_generic_argument(type_name.get(argument_start..index)?))
                    .flatten();
            }
            b'>' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn valid_generic_argument(argument: &str) -> Option<&str> {
    let argument = argument.trim();
    (!argument.is_empty() && !argument.starts_with('?')).then_some(argument)
}

fn array_element_type(type_name: &str) -> Option<&str> {
    type_name.trim().strip_suffix("[]").map(str::trim_end)
}

fn erase_java_type_syntax(type_name: &str) -> String {
    let mut result = String::with_capacity(type_name.len());
    let mut generic_depth = 0usize;
    for character in type_name.trim().chars() {
        match character {
            '<' => generic_depth += 1,
            '>' => generic_depth = generic_depth.saturating_sub(1),
            '[' if generic_depth == 0 => break,
            _ if generic_depth == 0 => result.push(character),
            _ => {}
        }
    }
    result.trim().to_owned()
}

fn merge_index_map(
    target: &mut BTreeMap<String, Vec<usize>>,
    source: BTreeMap<String, Vec<usize>>,
    offset: usize,
) {
    for (key, indexes) in source {
        target
            .entry(key)
            .or_default()
            .extend(indexes.into_iter().map(|index| index + offset));
    }
}

fn method_dispatch_signature(method: &JavaMethod) -> String {
    let parameters = method
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
    format!("{}({parameters})", method.name)
}

fn literal_java_type(value: &str) -> &'static str {
    let value = value.trim();
    if matches!(value, "true" | "false") {
        return "boolean";
    }
    if value == "null" {
        return "null";
    }
    if value.starts_with('"') {
        return "java.lang.String";
    }
    if value.as_bytes().first() == Some(&39) {
        return "char";
    }
    let lower = value.to_ascii_lowercase();
    if lower.ends_with('f') {
        return "float";
    }
    let floating = lower.contains('.')
        || lower.contains('p')
        || (!lower.starts_with("0x") && lower.contains('e'));
    if floating {
        return "double";
    }
    if lower.ends_with('l') {
        return "long";
    }
    "int"
}

fn java_type_match_score(actual: &str, expected: &str) -> u32 {
    if actual == expected {
        return 8;
    }
    if actual == "null" && !is_java_primitive(expected) {
        return 1;
    }
    if java_primitive_widens_to(actual, expected) {
        return 4;
    }
    0
}

fn is_java_primitive(type_name: &str) -> bool {
    matches!(
        type_name,
        "boolean" | "byte" | "short" | "char" | "int" | "long" | "float" | "double"
    )
}

fn java_primitive_widens_to(actual: &str, expected: &str) -> bool {
    match actual {
        "byte" => matches!(expected, "short" | "int" | "long" | "float" | "double"),
        "short" | "char" => matches!(expected, "int" | "long" | "float" | "double"),
        "int" => matches!(expected, "long" | "float" | "double"),
        "long" => matches!(expected, "float" | "double"),
        "float" => expected == "double",
        _ => false,
    }
}

fn unique_method<'a>(mut methods: impl Iterator<Item = &'a JavaMethod>) -> Option<&'a JavaMethod> {
    let method = methods.next()?;
    if methods.next().is_some() {
        return None;
    }
    Some(method)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JavaAnalysis {
    pub bindings: JavaBindings,
    pub inventory: JavaInventory,
    pub methods: Vec<JavaMethod>,
    pub fields: Vec<JavaField>,
    pub types: Vec<JavaTypeDeclaration>,
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
    let mut fields = Vec::new();
    collect_fields(
        tree.root_node(),
        source,
        path,
        artifact,
        version,
        &bindings,
        &mut fields,
    );
    let mut types = Vec::new();
    collect_type_declarations(tree.root_node(), source, &bindings, &mut types);
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
        fields,
        types,
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

fn qualified_reference_name(expression: &JavaExpression) -> Option<String> {
    match expression {
        JavaExpression::Reference(reference) => Some(reference.clone()),
        JavaExpression::FieldAccess { receiver, field } => {
            Some(format!("{}.{}", qualified_reference_name(receiver)?, field))
        }
        JavaExpression::Parenthesized(inner) => qualified_reference_name(inner),
        _ => None,
    }
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

fn collect_type_declarations(
    node: Node<'_>,
    source: &str,
    bindings: &JavaBindings,
    declarations: &mut Vec<JavaTypeDeclaration>,
) {
    let named_type = matches!(
        node.kind(),
        "class_declaration" | "interface_declaration" | "enum_declaration" | "record_declaration"
    );
    let anonymous_type = is_anonymous_object_creation(node);
    if (named_type || anonymous_type)
        && let Some(owner) = enclosing_type_name(node, source, bindings)
    {
        let mut direct_supertypes = Vec::new();
        if anonymous_type {
            if let Some(type_node) = node.child_by_field_name("type") {
                direct_supertypes.push(resolve_type_reference(
                    bindings,
                    &node_source(type_node, source),
                ));
            }
        } else {
            if let Some(superclass) = node.child_by_field_name("superclass")
                && let Some(type_node) = superclass.named_child(0)
            {
                direct_supertypes.push(resolve_type_reference(
                    bindings,
                    &node_source(type_node, source),
                ));
            }
            if let Some(interfaces) = node.child_by_field_name("interfaces") {
                collect_type_list(interfaces, source, bindings, &mut direct_supertypes);
            }
            if node.kind() == "interface_declaration" {
                let mut cursor = node.walk();
                for child in node
                    .named_children(&mut cursor)
                    .filter(|child| child.kind() == "extends_interfaces")
                {
                    collect_type_list(child, source, bindings, &mut direct_supertypes);
                }
            }
        }
        declarations.push(JavaTypeDeclaration {
            owner,
            direct_supertypes,
        });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_type_declarations(child, source, bindings, declarations);
    }
}

fn collect_type_list(
    node: Node<'_>,
    source: &str,
    bindings: &JavaBindings,
    output: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_list" {
            let mut type_cursor = child.walk();
            output.extend(child.named_children(&mut type_cursor).map(|type_node| {
                resolve_type_reference(bindings, &node_source(type_node, source))
            }));
        }
    }
}

fn collect_fields(
    node: Node<'_>,
    source: &str,
    path: &str,
    artifact: &str,
    version: Option<&str>,
    bindings: &JavaBindings,
    fields: &mut Vec<JavaField>,
) {
    if node.kind() == "enum_constant" {
        if let Some(name_node) = node.child_by_field_name("name")
            && let Ok(name) = node_text(name_node, source)
            && let Some(owner) = enclosing_type_name(node, source, bindings)
        {
            fields.push(JavaField {
                owner: owner.clone(),
                name: name.to_owned(),
                type_name: owner,
                is_static: true,
                origin: origin_for(node, Some(name), path, artifact, version),
            });
        }
        return;
    }
    if node.kind() == "field_declaration" {
        if let Some(type_node) = node.child_by_field_name("type")
            && let Ok(type_name) = node_text(type_node, source)
            && let Some(owner) = enclosing_type_name(node, source, bindings)
        {
            let type_name = resolve_type_reference(bindings, type_name.trim());
            let is_static = declaration_is_static(node, source);
            let mut cursor = node.walk();
            for declarator in node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "variable_declarator")
            {
                if let Some(name_node) = declarator.child_by_field_name("name")
                    && let Ok(name) = node_text(name_node, source)
                {
                    fields.push(JavaField {
                        owner: owner.clone(),
                        name: name.to_owned(),
                        type_name: type_name.clone(),
                        is_static,
                        origin: origin_for(declarator, Some(name), path, artifact, version),
                    });
                }
            }
        }
        return;
    }
    if matches!(
        node.kind(),
        "method_declaration" | "constructor_declaration" | "lambda_expression"
    ) {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_fields(child, source, path, artifact, version, bindings, fields);
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
    if matches!(
        node.kind(),
        "method_declaration" | "constructor_declaration"
    ) && let Some(method) = method_from_node(node, source, path, artifact, version, bindings)
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
    let parameters_node = node.child_by_field_name("parameters")?;
    let source_name = node_text(name_node, source).ok()?.to_owned();
    let owner = enclosing_type_name(node, source, bindings)?;
    let (name, return_type, is_static) = if node.kind() == "constructor_declaration" {
        (String::from("<init>"), owner.clone(), false)
    } else {
        let return_type_node = node.child_by_field_name("type")?;
        (
            source_name.clone(),
            resolve_type_reference(bindings, node_text(return_type_node, source).ok()?.trim()),
            method_is_static(node, source),
        )
    };
    let parameters = method_parameters(parameters_node, source, bindings);
    let mut returns = Vec::new();
    let mut local_variables = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        collect_returns(body, source, path, artifact, version, &name, &mut returns);
        collect_local_variables(body, source, bindings, &mut local_variables);
    }
    Some(JavaMethod {
        owner,
        name: name.clone(),
        return_type,
        parameters,
        local_variables,
        is_static,
        returns,
        origin: origin_for(node, Some(&source_name), path, artifact, version),
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
                type_name: resolve_type_reference(
                    bindings,
                    node_text(type_node, source).ok()?.trim(),
                ),
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

fn is_anonymous_object_creation(node: Node<'_>) -> bool {
    if node.kind() != "object_creation_expression" {
        return false;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == "class_body")
}

fn enclosing_type_name(node: Node<'_>, source: &str, bindings: &JavaBindings) -> Option<String> {
    let mut names = Vec::new();
    let mut current = Some(node);
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
            names.push(name.to_owned());
        } else if is_anonymous_object_creation(parent) {
            names.push(format!("$anon${}", parent.start_byte()));
        }
        current = parent.parent();
    }
    if names.is_empty() {
        return None;
    }
    names.reverse();
    let owner = names.join(".");
    Some(
        bindings
            .package
            .as_ref()
            .map_or(owner.clone(), |package| format!("{package}.{owner}")),
    )
}

fn method_is_static(node: Node<'_>, source: &str) -> bool {
    declaration_is_static(node, source)
}

fn declaration_is_static(node: Node<'_>, source: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == "modifiers")
        .and_then(|modifiers| node_text(modifiers, source).ok())
        .is_some_and(|modifiers| modifiers.split_whitespace().any(|token| token == "static"))
}

fn collect_local_variables(
    node: Node<'_>,
    source: &str,
    bindings: &JavaBindings,
    variables: &mut Vec<JavaLocalVariable>,
) {
    if node.kind() == "enhanced_for_statement" {
        if let (Some(type_node), Some(name_node), Some(body)) = (
            node.child_by_field_name("type"),
            node.child_by_field_name("name"),
            node.child_by_field_name("body"),
        ) && let (Ok(type_name), Ok(name)) =
            (node_text(type_node, source), node_text(name_node, source))
        {
            variables.push(JavaLocalVariable {
                type_name: resolve_type_reference(bindings, type_name.trim()),
                name: name.to_owned(),
                declaration_byte: name_node.start_byte(),
                scope_end_byte: body.end_byte(),
            });
            collect_local_variables(body, source, bindings, variables);
        }
        return;
    }
    if node.kind() == "local_variable_declaration" {
        if let Some(type_node) = node.child_by_field_name("type")
            && let Ok(type_name) = node_text(type_node, source)
        {
            let declared_type = resolve_type_reference(bindings, type_name.trim());
            let scope_end_byte = lexical_scope_end(node);
            let mut cursor = node.walk();
            for declarator in node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "variable_declarator")
            {
                if let Some(name_node) = declarator.child_by_field_name("name")
                    && let Ok(name) = node_text(name_node, source)
                {
                    let type_name = if declared_type == "var" {
                        inferred_var_type(declarator, source, bindings)
                            .unwrap_or_else(|| declared_type.clone())
                    } else {
                        declared_type.clone()
                    };
                    variables.push(JavaLocalVariable {
                        type_name,
                        name: name.to_owned(),
                        declaration_byte: declarator.start_byte(),
                        scope_end_byte,
                    });
                }
            }
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
        collect_local_variables(child, source, bindings, variables);
    }
}

fn inferred_var_type(
    declarator: Node<'_>,
    source: &str,
    bindings: &JavaBindings,
) -> Option<String> {
    let value = declarator.child_by_field_name("value")?;
    match value.kind() {
        "object_creation_expression" => {
            let type_node = value.child_by_field_name("type")?;
            Some(resolve_type_reference(
                bindings,
                node_text(type_node, source).ok()?.trim(),
            ))
        }
        "array_creation_expression" => {
            let type_node = value.child_by_field_name("type")?;
            Some(format!(
                "{}[]",
                resolve_type_reference(bindings, node_text(type_node, source).ok()?.trim())
            ))
        }
        _ => None,
    }
}

fn lexical_scope_end(node: Node<'_>) -> usize {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "block"
                | "constructor_body"
                | "for_statement"
                | "enhanced_for_statement"
                | "while_statement"
                | "do_statement"
                | "switch_block"
                | "catch_clause"
        ) {
            return parent.end_byte();
        }
        current = parent.parent();
    }
    node.end_byte()
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
        JavaExpression::FieldAccess { receiver, field } => JavaExpression::FieldAccess {
            receiver: Box::new(substitute_references(receiver, replacements)),
            field: field.clone(),
        },
        JavaExpression::Array {
            element_type,
            elements,
        } => JavaExpression::Array {
            element_type: element_type.clone(),
            elements: elements
                .iter()
                .map(|value| substitute_references(value, replacements))
                .collect(),
        },
        JavaExpression::ArrayAccess { array, index } => JavaExpression::ArrayAccess {
            array: Box::new(substitute_references(array, replacements)),
            index: Box::new(substitute_references(index, replacements)),
        },
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
        JavaExpression::Ternary {
            condition,
            consequence,
            alternative,
        } => JavaExpression::Ternary {
            condition: Box::new(substitute_references(condition, replacements)),
            consequence: Box::new(substitute_references(consequence, replacements)),
            alternative: Box::new(substitute_references(alternative, replacements)),
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
    let nodes: Vec<Node<'_>> = arguments
        .named_children(&mut cursor)
        .filter(|node| !is_comment_node(*node))
        .collect();
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
        "identifier" | "scoped_identifier" => JavaExpression::Reference(node_source(node, source)),
        "field_access" => expression_field_access(node, source),
        "array_creation_expression" => expression_array_creation(node, source),
        "array_initializer" => expression_array_initializer(node, source, None),
        "array_access" => expression_array_access(node, source),
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
        "ternary_expression" => expression_ternary(node, source),
        "parenthesized_expression" => node.named_child(0).map_or_else(
            || JavaExpression::Raw(node_source(node, source)),
            |child| JavaExpression::Parenthesized(Box::new(expression(child, source))),
        ),
        _ => JavaExpression::Raw(node_source(node, source)),
    }
}

fn expression_array_creation(node: Node<'_>, source: &str) -> JavaExpression {
    let Some(type_node) = node.child_by_field_name("type") else {
        return JavaExpression::Raw(node_source(node, source));
    };
    let element_type = node_source(type_node, source);
    node.child_by_field_name("value").map_or_else(
        || JavaExpression::Array {
            element_type: Some(element_type.clone()),
            elements: Vec::new(),
        },
        |initializer| expression_array_initializer(initializer, source, Some(element_type.clone())),
    )
}

fn expression_array_initializer(
    node: Node<'_>,
    source: &str,
    element_type: Option<String>,
) -> JavaExpression {
    let mut cursor = node.walk();
    JavaExpression::Array {
        element_type,
        elements: node
            .named_children(&mut cursor)
            .filter(|child| !is_comment_node(*child))
            .map(|child| expression(child, source))
            .collect(),
    }
}

fn expression_array_access(node: Node<'_>, source: &str) -> JavaExpression {
    let (Some(array), Some(index)) = (
        node.child_by_field_name("array"),
        node.child_by_field_name("index"),
    ) else {
        return JavaExpression::Raw(node_source(node, source));
    };
    JavaExpression::ArrayAccess {
        array: Box::new(expression(array, source)),
        index: Box::new(expression(index, source)),
    }
}

fn expression_field_access(node: Node<'_>, source: &str) -> JavaExpression {
    let (Some(object), Some(field)) = (
        node.child_by_field_name("object"),
        node.child_by_field_name("field"),
    ) else {
        return JavaExpression::Raw(node_source(node, source));
    };
    JavaExpression::FieldAccess {
        receiver: Box::new(expression(object, source)),
        field: node_source(field, source),
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

fn expression_ternary(node: Node<'_>, source: &str) -> JavaExpression {
    let (Some(condition), Some(consequence), Some(alternative)) = (
        node.child_by_field_name("condition"),
        node.child_by_field_name("consequence"),
        node.child_by_field_name("alternative"),
    ) else {
        return JavaExpression::Raw(node_source(node, source));
    };
    JavaExpression::Ternary {
        condition: Box::new(expression(condition, source)),
        consequence: Box::new(expression(consequence, source)),
        alternative: Box::new(expression(alternative, source)),
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

fn is_comment_node(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
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
        let by_owner = index
            .resolve_method("demo.Factory", "get", 1)
            .ok_or_else(|| anyhow::anyhow!("owner method not resolved"))?;
        assert_eq!(by_owner.parameters.len(), 1);
        Ok(())
    }

    #[test]
    fn resolves_enclosing_parameter_type_at_call_site() -> anyhow::Result<()> {
        let source = r"
            package demo;
            class Factory {
                void consume(Widget widget) { sink(widget.get(1)); }
            }
        ";
        let analysis = analyze_source("Factory.java", "demo", None, source)?;
        let call = analysis
            .call_chains
            .iter()
            .find(|chain| chain.calls.iter().any(|call| call.name == "sink"))
            .ok_or_else(|| anyhow::anyhow!("call missing"))?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        assert_eq!(
            index.parameter_type_at(
                "Factory.java",
                call.origin.byte_start.unwrap_or(0),
                "widget"
            ),
            Some("Widget")
        );
        Ok(())
    }

    #[test]
    fn resolves_nearest_in_scope_local_variable_type() -> anyhow::Result<()> {
        let source = r"
            package demo;
            class Factory {
                void consume(Widget parameter) {
                    Other widget = make();
                    sink(widget.get(1));
                }
            }
        ";
        let analysis = analyze_source("Factory.java", "demo", None, source)?;
        let call = analysis
            .call_chains
            .iter()
            .find(|chain| chain.calls.iter().any(|call| call.name == "sink"))
            .ok_or_else(|| anyhow::anyhow!("call missing"))?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let position = call.origin.byte_start.unwrap_or(0);
        assert_eq!(
            index.reference_type_at("Factory.java", position, "widget"),
            Some("Other")
        );
        assert_eq!(
            index.reference_type_at("Factory.java", position, "parameter"),
            Some("Widget")
        );
        Ok(())
    }

    #[test]
    fn parses_array_creation_and_access_structurally() -> anyhow::Result<()> {
        let source = r#"
            package demo;
            import net.minecraft.item.ItemStack;
            class Recipe {
                static final ItemStack[] ITEMS = new ItemStack[4];
                void run() {
                    sink(
                        // recipe-key comment is not an argument
                        new Object[] { "dustIron", /* amount */ 4 });
                    sink(new ItemStack[] { ITEMS[0], ITEMS[1] });
                }
            }
        "#;
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let sink_calls: Vec<_> = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .filter(|call| call.name == "sink")
            .collect();
        assert_eq!(sink_calls.len(), 2);
        assert_eq!(sink_calls[0].arguments.len(), 1);
        let Some(super::JavaExpression::Array {
            element_type: Some(element_type),
            elements,
        }) = sink_calls[0].argument_expressions.first()
        else {
            return Err(anyhow::anyhow!("Object array was not structural"));
        };
        assert_eq!(element_type, "Object");
        assert_eq!(elements.len(), 2);

        let Some(super::JavaExpression::Array {
            element_type: Some(element_type),
            elements,
        }) = sink_calls[1].argument_expressions.first()
        else {
            return Err(anyhow::anyhow!("ItemStack array was not structural"));
        };
        assert_eq!(element_type, "ItemStack");
        assert!(matches!(
            elements.first(),
            Some(super::JavaExpression::ArrayAccess { .. })
        ));

        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        assert_eq!(
            index.expression_type_at(
                "Recipe.java",
                sink_calls[1].byte_start,
                &elements[0],
                &analysis.bindings,
            ),
            Some(String::from("net.minecraft.item.ItemStack"))
        );
        Ok(())
    }

    #[test]
    fn infers_standard_list_get_type() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import java.util.List;
            import net.minecraft.item.ItemStack;
            class Recipe {
                void run(List<ItemStack> items) { sink(items.get(0)); }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        assert_eq!(
            index.expression_type_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings
            ),
            Some(String::from("ItemStack"))
        );
        Ok(())
    }

    #[test]
    fn infers_objects_require_non_null_type() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import java.util.Objects;
            class ItemStack {}
            class Recipe {
                void run(ItemStack item) { sink(Objects.requireNonNull(item)); }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        assert_eq!(
            index.expression_type_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings
            ),
            Some(String::from("demo.ItemStack"))
        );
        Ok(())
    }

    #[test]
    fn infers_optional_and_map_get_types() -> anyhow::Result<()> {
        let source = r#"
            package demo;
            import java.util.Map;
            import java.util.Optional;
            class ItemStack {}
            class Recipe {
                void run(Optional<ItemStack> optional, Map<String, ItemStack> items) {
                    sink(optional.get());
                    sink(items.get("key"));
                }
            }
        "#;
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink_calls: Vec<_> = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .filter(|call| call.name == "sink")
            .collect();
        assert_eq!(sink_calls.len(), 2);
        for sink in sink_calls {
            let expression = sink
                .argument_expressions
                .first()
                .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
            assert_eq!(
                index.expression_type_at(
                    "Recipe.java",
                    sink.byte_start,
                    expression,
                    &analysis.bindings
                ),
                Some(String::from("demo.ItemStack"))
            );
        }
        Ok(())
    }

    #[test]
    fn infers_standard_collection_to_array_type() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import java.util.List;
            import net.minecraft.item.ItemStack;
            class Recipe {
                void run(List<ItemStack> items) {
                    sink(items.toArray(new ItemStack[0]));
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        assert_eq!(
            index.expression_type_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings
            ),
            Some(String::from("net.minecraft.item.ItemStack[]"))
        );
        Ok(())
    }

    #[test]
    fn resolves_recipe_value_provenance() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import static demo.Registry.IMPORTED;
            import net.minecraft.item.ItemStack;
            class Registry { static ItemStack IMPORTED; }
            class Recipe {
                ItemStack member;
                void run(ItemStack parameter) {
                    ItemStack local = parameter;
                    sink(parameter);
                    sink(local);
                    sink(member);
                    sink(IMPORTED);
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink_calls: Vec<_> = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .filter(|call| call.name == "sink")
            .collect();
        let expected = [
            super::JavaValueKind::Parameter,
            super::JavaValueKind::Local,
            super::JavaValueKind::Field,
            super::JavaValueKind::StaticImportedField,
        ];
        for (call, expected_kind) in sink_calls.iter().zip(expected) {
            let Some(super::JavaExpression::Reference(reference)) =
                call.argument_expressions.first()
            else {
                return Err(anyhow::anyhow!("sink argument was not a reference"));
            };
            let value = index
                .resolve_value_at(
                    "Recipe.java",
                    call.byte_start,
                    reference,
                    &analysis.bindings,
                )
                .ok_or_else(|| anyhow::anyhow!("value was not resolved"))?;
            assert_eq!(value.kind, expected_kind);
            assert_eq!(value.type_name, "net.minecraft.item.ItemStack");
        }
        Ok(())
    }

    #[test]
    fn resolves_enum_constant_receiver_type() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import net.minecraft.item.ItemStack;
            enum Component {
                MOTOR;
                ItemStack get(int amount) { return null; }
            }
            class Recipe { void run() { sink(Component.MOTOR.get(4)); } }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        let (resolved, receiver_type) = index
            .resolve_call_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings,
            )
            .ok_or_else(|| anyhow::anyhow!("enum receiver was not resolved"))?;
        assert_eq!(resolved.owner, "demo.Component");
        assert_eq!(receiver_type.as_deref(), Some("demo.Component"));
        Ok(())
    }

    #[test]
    fn resolves_enclosing_and_static_imported_field_receivers() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import static demo.Registry.IMPORTED;
            import net.minecraft.item.ItemStack;
            class Holder { ItemStack get(int amount) { return null; } }
            class Registry { static final Holder IMPORTED = new Holder(); }
            class Recipe {
                Holder member = new Holder();
                void run() {
                    sink(member.get(1));
                    sink(IMPORTED.get(2));
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let calls: Vec<_> = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .filter(|call| call.name == "sink")
            .collect();
        assert_eq!(calls.len(), 2);
        for call in calls {
            let expression = call
                .argument_expressions
                .first()
                .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
            let (resolved, receiver_type) = index
                .resolve_call_at(
                    "Recipe.java",
                    call.byte_start,
                    expression,
                    &analysis.bindings,
                )
                .ok_or_else(|| anyhow::anyhow!("field receiver was not resolved"))?;
            assert_eq!(resolved.owner, "demo.Holder");
            assert_eq!(resolved.name, "get");
            assert_eq!(receiver_type.as_deref(), Some("demo.Holder"));
        }
        Ok(())
    }

    #[test]
    fn infers_var_type_from_direct_constructor_initializer() -> anyhow::Result<()> {
        let source = r"
            package demo;
            class OreDictItemStack { void marker() {} }
            class Recipe {
                void run() {
                    var input = new OreDictItemStack();
                    sink(input);
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let value = index
            .resolve_value_at("Recipe.java", sink.byte_start, "input", &analysis.bindings)
            .ok_or_else(|| anyhow::anyhow!("inferred var missing"))?;
        assert_eq!(value.type_name, "demo.OreDictItemStack");
        Ok(())
    }

    #[test]
    fn resolves_values_inside_constructor_scope() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import net.minecraft.item.ItemStack;
            class Recipe {
                Recipe(ItemStack parameter) {
                    ItemStack local = parameter;
                    sink(parameter);
                    sink(local);
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let calls: Vec<_> = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .filter(|call| call.name == "sink")
            .collect();
        assert_eq!(calls.len(), 2);
        for (call, expected) in calls.iter().zip(["parameter", "local"]) {
            let value = index
                .resolve_value_at("Recipe.java", call.byte_start, expected, &analysis.bindings)
                .ok_or_else(|| anyhow::anyhow!("constructor value missing"))?;
            assert_eq!(value.type_name, "net.minecraft.item.ItemStack");
        }
        Ok(())
    }

    #[test]
    fn resolves_enhanced_for_variable_scope() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import java.util.List;
            import net.minecraft.item.ItemStack;
            class Recipe {
                void run(List<ItemStack> items) {
                    for (ItemStack item : items) { sink(item); }
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let value = index
            .resolve_value_at("Recipe.java", sink.byte_start, "item", &analysis.bindings)
            .ok_or_else(|| anyhow::anyhow!("enhanced-for value missing"))?;
        assert_eq!(value.type_name, "net.minecraft.item.ItemStack");
        Ok(())
    }

    #[test]
    fn resolves_lexically_visible_nested_type() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import net.minecraft.item.ItemStack;
            class ItemList { ItemStack get(long amount) { return null; } }
            class Outer {
                enum ComponentType {
                    MOTOR;
                    ItemList getComponent(int tier) { return null; }
                }
                void run(int tier) { sink(ComponentType.MOTOR.getComponent(tier).get(4)); }
            }
        ";
        let analysis = analyze_source("Outer.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        let (resolved, receiver_type) = index
            .resolve_call_at(
                "Outer.java",
                sink.byte_start,
                expression,
                &analysis.bindings,
            )
            .ok_or_else(|| anyhow::anyhow!("nested lexical type was not resolved"))?;
        assert_eq!(resolved.owner, "demo.ItemList");
        assert_eq!(receiver_type.as_deref(), Some("demo.ItemList"));
        Ok(())
    }

    #[test]
    fn resolves_nested_type_field_chain() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import net.minecraft.item.ItemStack;
            class Holder { ItemStack get(int amount) { return null; } }
            class Outer {
                static class Inner { static final Holder VALUE = new Holder(); }
            }
            class Recipe { void run() { sink(Outer.Inner.VALUE.get(4)); } }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        let nested_field = index
            .resolve_field("demo.Outer.Inner", "VALUE", Some(true))
            .ok_or_else(|| anyhow::anyhow!("nested VALUE field was not indexed"))?;
        assert_eq!(nested_field.type_name, "Holder");
        let super::JavaExpression::Call {
            receiver: Some(receiver),
            ..
        } = expression
        else {
            return Err(anyhow::anyhow!("sink argument was not a receiver call"));
        };
        assert_eq!(
            index.expression_type_at("Recipe.java", sink.byte_start, receiver, &analysis.bindings),
            Some(String::from("demo.Holder"))
        );
        let direct = index
            .resolve_instance_method("demo.Holder", "get", 1)
            .ok_or_else(|| anyhow::anyhow!("direct Holder#get lookup failed"))?;
        assert_eq!(direct.owner, "demo.Holder");
        let (resolved, receiver_type) = index
            .resolve_call_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings,
            )
            .ok_or_else(|| anyhow::anyhow!("nested field chain was not resolved"))?;
        assert_eq!(resolved.owner, "demo.Holder");
        assert_eq!(receiver_type.as_deref(), Some("demo.Holder"));
        Ok(())
    }

    #[test]
    fn resolves_member_type_inside_anonymous_subclass() -> anyhow::Result<()> {
        let source = r"
            package demo;
            class ItemStack {}
            abstract class Base {
                protected static class Iter { ItemStack nextItem() { return null; } }
                protected abstract void apply(Iter iterator);
            }
            class Recipe {
                void run() {
                    new Base() {
                        @Override protected void apply(Iter iterator) {
                            sink(iterator.nextItem());
                        }
                    };
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        let (resolved, receiver_type) = index
            .resolve_call_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings,
            )
            .ok_or_else(|| anyhow::anyhow!("anonymous inherited member type was not resolved"))?;
        assert_eq!(resolved.owner, "demo.Base.Iter");
        assert_eq!(receiver_type.as_deref(), Some("demo.Base.Iter"));
        Ok(())
    }

    #[test]
    fn resolves_inherited_instance_method() -> anyhow::Result<()> {
        let source = r"
            package demo;
            class ItemStack {}
            class BaseTool { ItemStack build(int amount) { return null; } }
            class Tool extends BaseTool {}
            class Recipe { void run(Tool tool) { sink(tool.build(4)); } }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        let (resolved, receiver_type) = index
            .resolve_call_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings,
            )
            .ok_or_else(|| anyhow::anyhow!("inherited method was not resolved"))?;
        assert_eq!(resolved.owner, "demo.BaseTool");
        assert_eq!(receiver_type.as_deref(), Some("demo.Tool"));
        Ok(())
    }

    #[test]
    fn resolves_static_varargs_overload_with_type_evidence() -> anyhow::Result<()> {
        let source = r"
            package demo;
            class ItemStack {}
            class Materials {}
            class Helper {
                static ItemStack[] convert(Materials material, ItemStack... outputs) { return null; }
                static ItemStack[] convert(ItemStack... outputs) { return null; }
            }
            class Recipe {
                void run(Materials material, ItemStack stack) {
                    sink(Helper.convert(material, stack));
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        let (resolved, _) = index
            .resolve_call_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings,
            )
            .ok_or_else(|| anyhow::anyhow!("overloaded static call was not resolved"))?;
        assert_eq!(resolved.owner, "demo.Helper");
        assert_eq!(resolved.parameters.len(), 2);
        assert_eq!(resolved.parameters[0].type_name, "Materials");
        Ok(())
    }

    #[test]
    fn resolves_primitive_widening_overload_with_type_evidence() -> anyhow::Result<()> {
        let source = r"
            package demo;
            class ItemStack {}
            class Holder {
                ItemStack get(long amount, Object... replacements) { return null; }
                ItemStack get(String key, Object... replacements) { return null; }
            }
            class Recipe { void run(Holder holder) { sink(holder.get(4)); } }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        let (resolved, _) = index
            .resolve_call_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings,
            )
            .ok_or_else(|| anyhow::anyhow!("primitive overload was not resolved"))?;
        assert_eq!(resolved.parameters[0].type_name, "long");
        Ok(())
    }

    #[test]
    fn parses_and_types_ternary_expression() -> anyhow::Result<()> {
        let source = r"
            package demo;
            class ItemStack {}
            class Recipe {
                void run(boolean choose, ItemStack left, ItemStack right) {
                    sink(choose ? left : right);
                }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        assert!(matches!(expression, super::JavaExpression::Ternary { .. }));
        assert_eq!(
            index.expression_type_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings
            ),
            Some(String::from("demo.ItemStack"))
        );
        Ok(())
    }

    #[test]
    fn resolves_static_call_and_typed_field_chain() -> anyhow::Result<()> {
        let source = r"
            package demo;
            import net.minecraft.item.ItemStack;
            class Material { ItemStack getDust(int amount) { return null; } }
            class Materials {
                public final Material IRON = new Material();
                public static Materials getInstance() { return null; }
            }
            class Recipe {
                void run() { sink(Materials.getInstance().IRON.getDust(4)); }
            }
        ";
        let analysis = analyze_source("Recipe.java", "demo", None, source)?;
        let mut index = super::JavaProjectIndex::default();
        index.add_analysis(&analysis);
        let sink = analysis
            .call_chains
            .iter()
            .flat_map(|chain| &chain.calls)
            .find(|call| call.name == "sink")
            .ok_or_else(|| anyhow::anyhow!("sink call missing"))?;
        let expression = sink
            .argument_expressions
            .first()
            .ok_or_else(|| anyhow::anyhow!("sink argument missing"))?;
        let (resolved, receiver_type) = index
            .resolve_call_at(
                "Recipe.java",
                sink.byte_start,
                expression,
                &analysis.bindings,
            )
            .ok_or_else(|| anyhow::anyhow!("field chain was not resolved"))?;
        assert_eq!(resolved.owner, "demo.Material");
        assert_eq!(resolved.name, "getDust");
        assert_eq!(resolved.return_type, "net.minecraft.item.ItemStack");
        assert_eq!(receiver_type.as_deref(), Some("demo.Material"));
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
