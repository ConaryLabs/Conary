// conary-core/src/resolver/mod.rs

//! Dependency resolution and conflict detection
//!
//! This module provides dependency graph construction, topological sorting,
//! cycle detection, and conflict resolution for package dependencies.
//!
//! It also provides component-level resolution for independent component
//! installation and removal safety checking.

pub mod canonical;
mod component_resolver;
mod conflict;
mod engine;
mod graph;
mod plan;
pub mod provider;
pub mod sat;

pub use component_resolver::{
    ComponentResolutionPlan, ComponentResolver, ComponentSpec, MissingComponent,
};
pub use conflict::Conflict;
pub use engine::Resolver;
pub use graph::{DependencyEdge, DependencyGraph, GraphStats, PackageNode};
pub use plan::{MissingDependency, ResolutionPlan};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::{RpmVersion, VersionConstraint};

    fn make_version(s: &str) -> RpmVersion {
        RpmVersion::parse(s).unwrap()
    }

    #[test]
    fn test_graph_creation() {
        let graph = DependencyGraph::new();
        assert_eq!(graph.nodes.len(), 0);
        assert_eq!(graph.edges.len(), 0);
    }

    #[test]
    fn test_add_node() {
        let mut graph = DependencyGraph::new();
        let node = PackageNode::new("test-package".to_string(), make_version("1.0.0"));
        graph.add_node(node.clone());

        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.get_node("test-package"), Some(&node));
    }

    #[test]
    fn test_add_edge() {
        let mut graph = DependencyGraph::new();

        let node1 = PackageNode::new("package-a".to_string(), make_version("1.0.0"));
        let node2 = PackageNode::new("package-b".to_string(), make_version("2.0.0"));

        graph.add_node(node1);
        graph.add_node(node2);

        let edge = DependencyEdge {
            from: "package-a".to_string(),
            to: "package-b".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        };

        graph.add_edge(edge);

        let deps = graph.get_dependencies("package-a");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, "package-b");

        let dependents = graph.get_dependents("package-b");
        assert_eq!(dependents.len(), 1);
        assert_eq!(dependents[0], "package-a");
    }

    #[test]
    fn test_topological_sort_simple() {
        let mut graph = DependencyGraph::new();

        // Create a simple dependency chain: A -> B -> C
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        let sorted = graph.topological_sort().unwrap();

        // C should come before B, and B should come before A
        let pos_a = sorted.iter().position(|x| x == "A").unwrap();
        let pos_b = sorted.iter().position(|x| x == "B").unwrap();
        let pos_c = sorted.iter().position(|x| x == "C").unwrap();

        assert!(pos_c < pos_b);
        assert!(pos_b < pos_a);
    }

    #[test]
    fn test_topological_sort_diamond() {
        let mut graph = DependencyGraph::new();

        // Diamond dependency:
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D

        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("D".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "D".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "C".to_string(),
            to: "D".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        let sorted = graph.topological_sort().unwrap();

        // D should come before both B and C, and both B and C before A
        let pos_a = sorted.iter().position(|x| x == "A").unwrap();
        let pos_b = sorted.iter().position(|x| x == "B").unwrap();
        let pos_c = sorted.iter().position(|x| x == "C").unwrap();
        let pos_d = sorted.iter().position(|x| x == "D").unwrap();

        assert!(pos_d < pos_b);
        assert!(pos_d < pos_c);
        assert!(pos_b < pos_a);
        assert!(pos_c < pos_a);
    }

    #[test]
    fn test_cycle_detection_simple() {
        let mut graph = DependencyGraph::new();

        // Create a cycle: A -> B -> C -> A
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "C".to_string(),
            to: "A".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        let cycle = graph.detect_cycle();
        assert!(cycle.is_some());

        // Topological sort should fail
        let result = graph.topological_sort();
        assert!(result.is_err());
    }

    #[test]
    fn test_no_cycle() {
        let mut graph = DependencyGraph::new();

        // Create a DAG: A -> B -> C
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        let cycle = graph.detect_cycle();
        assert!(cycle.is_none());
    }

    #[test]
    fn test_check_constraints_satisfied() {
        let mut graph = DependencyGraph::new();

        graph.add_node(PackageNode::new("lib".to_string(), make_version("2.0.0")));
        graph.add_node(PackageNode::new("app".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        let lib_version = make_version("2.0.0");
        assert!(graph.check_constraints("lib", &lib_version).is_ok());
    }

    #[test]
    fn test_check_constraints_violated() {
        let mut graph = DependencyGraph::new();

        graph.add_node(PackageNode::new("lib".to_string(), make_version("0.5.0")));
        graph.add_node(PackageNode::new("app".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        let lib_version = make_version("0.5.0");
        assert!(graph.check_constraints("lib", &lib_version).is_err());
    }

    #[test]
    fn test_find_breaking_packages() {
        let mut graph = DependencyGraph::new();

        // Create dependency chain: lib <- app1 <- app2
        graph.add_node(PackageNode::new("lib".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("app1".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("app2".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app1".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "app2".to_string(),
            to: "app1".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        let breaking = graph.find_breaking_packages("lib");

        // Both app1 and app2 should break if lib is removed
        assert_eq!(breaking.len(), 2);
        assert!(breaking.contains(&"app1".to_string()));
        assert!(breaking.contains(&"app2".to_string()));
    }

    #[test]
    fn test_graph_stats() {
        let mut graph = DependencyGraph::new();

        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        let stats = graph.stats();
        assert_eq!(stats.total_packages, 3);
        assert_eq!(stats.total_dependencies, 2);
        assert_eq!(stats.max_dependencies, 2); // A has 2 dependencies
        assert_eq!(stats.max_dependents, 1); // B and C each have 1 dependent
    }

    // NOTE: Resolver-level tests (install, removal, conflicts) are in
    // resolver::sat::tests, which tests the production SAT-backed path
    // against a real DB. The tests above test the DependencyGraph data
    // structure directly.
}
