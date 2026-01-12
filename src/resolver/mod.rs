// src/resolver/mod.rs

//! Dependency resolution and conflict detection
//!
//! This module provides dependency graph construction, topological sorting,
//! cycle detection, and conflict resolution for package dependencies.

mod conflict;
mod graph;
mod plan;
mod resolver;

pub use conflict::Conflict;
pub use graph::{DependencyEdge, DependencyGraph, GraphStats, PackageNode};
pub use plan::{MissingDependency, ResolutionPlan};
pub use resolver::Resolver;

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
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
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
        });

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "D".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "C".to_string(),
            to: "D".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
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
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "C".to_string(),
            to: "A".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
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
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
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
        });

        graph.add_edge(DependencyEdge {
            from: "app2".to_string(),
            to: "app1".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
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
        });

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let stats = graph.stats();
        assert_eq!(stats.total_packages, 3);
        assert_eq!(stats.total_dependencies, 2);
        assert_eq!(stats.max_dependencies, 2); // A has 2 dependencies
        assert_eq!(stats.max_dependents, 1); // B and C each have 1 dependent
    }

    #[test]
    fn test_resolver_simple() {
        let mut graph = DependencyGraph::new();

        // Simple case: A depends on B
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.conflicts.len(), 0);
        assert_eq!(plan.missing.len(), 0);
        assert_eq!(plan.install_order.len(), 2);
    }

    #[test]
    fn test_resolver_missing_dependency() {
        let mut graph = DependencyGraph::new();

        // A depends on B, but B is not installed
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.missing.len(), 1);
        assert_eq!(plan.missing[0].name, "B");
        assert_eq!(plan.missing[0].required_by, vec!["A"]);
    }

    #[test]
    fn test_resolver_version_conflict() {
        let mut graph = DependencyGraph::new();

        // A depends on B >= 2.0.0, but B 1.0.0 is installed
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::parse(">= 2.0.0").unwrap(),
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.conflicts.len(), 1);
        match &plan.conflicts[0] {
            Conflict::UnsatisfiableConstraint {
                package,
                installed_version,
                required_constraint,
                required_by,
            } => {
                assert_eq!(package, "B");
                assert_eq!(installed_version, "1.0.0");
                assert_eq!(required_constraint, ">= 2.0.0");
                assert_eq!(required_by, "A");
            }
            _ => panic!("Expected UnsatisfiableConstraint"),
        }
    }

    #[test]
    fn test_resolver_circular_dependency() {
        let mut graph = DependencyGraph::new();

        // Circular: A -> B -> C -> A
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "C".to_string(),
            to: "A".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.conflicts.len(), 1);
        match &plan.conflicts[0] {
            Conflict::CircularDependency { cycle } => {
                assert!(cycle.len() >= 3);
            }
            _ => panic!("Expected CircularDependency"),
        }
    }

    #[test]
    fn test_resolver_check_removal() {
        let mut graph = DependencyGraph::new();

        // lib <- app1 <- app2
        graph.add_node(PackageNode::new("lib".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("app1".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("app2".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app1".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "app2".to_string(),
            to: "app1".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let breaking = resolver.check_removal("lib").unwrap();

        assert_eq!(breaking.len(), 2);
        assert!(breaking.contains(&"app1".to_string()));
        assert!(breaking.contains(&"app2".to_string()));
    }

    #[test]
    fn test_resolver_install_order() {
        let mut graph = DependencyGraph::new();

        // Complex dependency chain: app -> lib1 -> lib2
        //                                 -> lib3
        graph.add_node(PackageNode::new("app".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("lib1".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("lib2".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("lib3".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app".to_string(),
            to: "lib1".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "app".to_string(),
            to: "lib3".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "lib1".to_string(),
            to: "lib2".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.conflicts.len(), 0);
        assert_eq!(plan.missing.len(), 0);

        // lib2 and lib3 should come before lib1, lib1 before app
        let pos_app = plan.install_order.iter().position(|x| x == "app").unwrap();
        let pos_lib1 = plan.install_order.iter().position(|x| x == "lib1").unwrap();
        let pos_lib2 = plan.install_order.iter().position(|x| x == "lib2").unwrap();
        let pos_lib3 = plan.install_order.iter().position(|x| x == "lib3").unwrap();

        assert!(pos_lib2 < pos_lib1);
        assert!(pos_lib3 < pos_app);
        assert!(pos_lib1 < pos_app);
    }
}
