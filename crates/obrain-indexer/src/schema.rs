//! Le contrat cible de l'indexeur (RFC M6 §2.1bis).
//!
//! Reproduit les labels de nœuds et types d'arêtes de l'indexeur C
//! `codebase-memory-mcp` — le schéma OUVERT que M6 doit répliquer à
//! l'identique (équivalence a minima, décision utilisateur 2026-09-06).

use serde::{Deserialize, Serialize};

/// Labels de nœuds produits (contrat RFC M6 §2.1bis — 24 labels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum NodeKind {
    Project,
    Branch,
    Folder,
    File,
    Module,
    Function,
    Method,
    Class,
    Interface,
    Struct,
    Enum,
    Type,
    Field,
    Variable,
    Macro,
    Section,
    Route,
    Channel,
    EnvVar,
    Decorator,
    Resource,
    Package,
    Table,
    View,
    Model,
    Constructor,
}

impl NodeKind {
    pub const ALL: &'static [NodeKind] = &[
        NodeKind::Project,
        NodeKind::Branch,
        NodeKind::Folder,
        NodeKind::File,
        NodeKind::Module,
        NodeKind::Function,
        NodeKind::Method,
        NodeKind::Class,
        NodeKind::Interface,
        NodeKind::Struct,
        NodeKind::Enum,
        NodeKind::Type,
        NodeKind::Field,
        NodeKind::Variable,
        NodeKind::Macro,
        NodeKind::Section,
        NodeKind::Route,
        NodeKind::Channel,
        NodeKind::EnvVar,
        NodeKind::Decorator,
        NodeKind::Resource,
        NodeKind::Package,
        NodeKind::Table,
        NodeKind::View,
        NodeKind::Model,
        NodeKind::Constructor,
    ];

    /// Nom de label tel qu'écrit dans le graphe (str du contrat C).
    pub fn label(self) -> &'static str {
        match self {
            NodeKind::Project => "Project",
            NodeKind::Branch => "Branch",
            NodeKind::Folder => "Folder",
            NodeKind::File => "File",
            NodeKind::Module => "Module",
            NodeKind::Function => "Function",
            NodeKind::Method => "Method",
            NodeKind::Class => "Class",
            NodeKind::Interface => "Interface",
            NodeKind::Struct => "Struct",
            NodeKind::Enum => "Enum",
            NodeKind::Type => "Type",
            NodeKind::Field => "Field",
            NodeKind::Variable => "Variable",
            NodeKind::Macro => "Macro",
            NodeKind::Section => "Section",
            NodeKind::Route => "Route",
            NodeKind::Channel => "Channel",
            NodeKind::EnvVar => "EnvVar",
            NodeKind::Decorator => "Decorator",
            NodeKind::Resource => "Resource",
            NodeKind::Package => "Package",
            NodeKind::Table => "Table",
            NodeKind::View => "View",
            NodeKind::Model => "Model",
            NodeKind::Constructor => "Constructor",
        }
    }
}

/// Types d'arêtes produits (contrat RFC M6 §2.1bis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum EdgeKind {
    // Cœur — graphe de code
    Defines,
    DefinesMethod,
    Imports,
    Calls,
    Usage,
    CallReference,
    ContainsFile,
    ContainsFolder,
    HasBranch,
    Inherits,
    Implements,
    Override,
    Decorates,
    Throws,
    Raises,
    Writes,
    Reads,
    Emits,
    ListensOn,
    Tests,
    TestsFile,
    // Enrichissement
    SimilarTo,
    SemanticallyRelated,
    FileChangesWith,
    Configures,
    // Web / routes
    HttpCalls,
    AsyncCalls,
    Handles,
    InfraMaps,
    DataFlows,
    // Cross-repo
    CrossHttpCalls,
    CrossAsyncCalls,
    CrossChannel,
    CrossGrpcCalls,
}

impl EdgeKind {
    /// Nom du type d'arête tel qu'écrit dans le graphe (str du contrat C).
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::Defines => "DEFINES",
            EdgeKind::DefinesMethod => "DEFINES_METHOD",
            EdgeKind::Imports => "IMPORTS",
            EdgeKind::Calls => "CALLS",
            EdgeKind::Usage => "USAGE",
            EdgeKind::CallReference => "CALL_REFERENCE",
            EdgeKind::ContainsFile => "CONTAINS_FILE",
            EdgeKind::ContainsFolder => "CONTAINS_FOLDER",
            EdgeKind::HasBranch => "HAS_BRANCH",
            EdgeKind::Inherits => "INHERITS",
            EdgeKind::Implements => "IMPLEMENTS",
            EdgeKind::Override => "OVERRIDE",
            EdgeKind::Decorates => "DECORATES",
            EdgeKind::Throws => "THROWS",
            EdgeKind::Raises => "RAISES",
            EdgeKind::Writes => "WRITES",
            EdgeKind::Reads => "READS",
            EdgeKind::Emits => "EMITS",
            EdgeKind::ListensOn => "LISTENS_ON",
            EdgeKind::Tests => "TESTS",
            EdgeKind::TestsFile => "TESTS_FILE",
            EdgeKind::SimilarTo => "SIMILAR_TO",
            EdgeKind::SemanticallyRelated => "SEMANTICALLY_RELATED",
            EdgeKind::FileChangesWith => "FILE_CHANGES_WITH",
            EdgeKind::Configures => "CONFIGURES",
            EdgeKind::HttpCalls => "HTTP_CALLS",
            EdgeKind::AsyncCalls => "ASYNC_CALLS",
            EdgeKind::Handles => "HANDLES",
            EdgeKind::InfraMaps => "INFRA_MAPS",
            EdgeKind::DataFlows => "DATA_FLOWS",
            EdgeKind::CrossHttpCalls => "CROSS_HTTP_CALLS",
            EdgeKind::CrossAsyncCalls => "CROSS_ASYNC_CALLS",
            EdgeKind::CrossChannel => "CROSS_CHANNEL",
            EdgeKind::CrossGrpcCalls => "CROSS_GRPC_CALLS",
        }
    }
}
