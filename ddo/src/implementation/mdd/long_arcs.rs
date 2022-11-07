//! This is an adaptation of the vector based architecture which implements all
//! the pruning techniques that I have proposed in my PhD thesis (RUB, LocB, EBPO).
//! 
//! This implementation varies from the default one in that it implements long 
//! arcs in the decision diagrams. This might or might not be suitable for your
//! purpose.
use std::{collections::hash_map::Entry, hash::Hash, ops::Deref, sync::Arc};

use rustc_hash::FxHashMap;

use crate::{Decision, DecisionDiagram, CompilationInput, Problem, SubProblem, CompilationType, Completion, Reason};

use super::node_flags::NodeFlags;

/// The identifier of a node: it indicates the position of the referenced node 
/// in the ’nodes’ vector of the ’VectorBased’ structure.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct NodeId(usize);

/// The identifier of an edge: it indicates the position of the referenced edge 
/// in the ’edges’ vector of the ’VectorBased’ structure.
#[derive(Debug, Clone, Copy)]
struct EdgeId(usize);

/// Represents an effective node from the decision diagram
#[derive(Debug, Clone)]
struct Node<T> {
    /// The state associated with this node
    state: Arc<T>,
    /// The length of the longest path between the problem root and this
    /// specific node
    value: isize,
    /// The length of the longest path between this node and the terminal node.
    /// 
    /// ### Note
    /// This field is only ever populated after the MDD has been fully unrolled.
    value_bot: isize,
    /// The identifier of the last edge on the longest path between the problem 
    /// root and this node if it exists.
    best: Option<EdgeId>,
    /// The identifier of the latest edge having been added to the adjacency
    /// list of this node. (Edges, by themselves form a kind of linked structure)
    inbound: Option<EdgeId>,
    // The rough upper bound associated to this node
    rub: isize,
    /// A group of flag telling if the node is an exact node, if it is a relaxed
    /// node (helps to determine if the best path is an exact path) and if the
    /// node is reachable in a backwards traversal of the MDD starting at the
    /// terminal node.
    flags: NodeFlags,
}

/// Materializes one edge a.k.a arc from the decision diagram. It logically 
/// connects two nodes and annotates the link with a decision and a cost.
#[derive(Debug, Clone, Copy)]
struct Edge {
    /// The identifier of the node at the ∗∗source∗∗ of this edge.
    /// The destination end of this arc is not mentioned explicitly since it
    /// is simply the node having this edge in its inbound edges list.
    from: NodeId,
    /// This is the decision label associated to this edge. It gives the 
    /// information "what variable" is assigned to "what value".
    decision: Decision,
    /// This is the transition cost of making this decision from the state
    /// associated with the source node of this edge.
    cost: isize,
    /// This is a peculiarity of this design: a node does not maintain a 
    /// explicit adjacency list (only an optional edge id). The rest of the
    /// list is then encoded as a kind of ’linked’ list: each edge knows 
    /// the identifier of the next edge in the adjacency list (if there is
    /// one such edge).
    next: Option<EdgeId>,
}

/// The decision diagram in itself. This structure essentially keeps track
/// of the nodes composing the diagam as well as the edges connecting these
/// nodes in two vectors (enabling preallocation and good cache locality). 
/// In addition to that, it also keeps track of the path (root_pa) from the
/// problem root to the root of this decision diagram (explores a sub problem). 
/// 
/// # Note
/// This version of the decision diagram is one that implements long arcs.
/// 
/// # Exact Cutset
/// The exact cutset which is used in this implementation is the 
/// Last Exact Layer cutset (LEL).
/// 
/// # Performance
/// While the implementation of this MDD with long arcs is extremely similar
/// to that of the vector based mdd without long arcs; the potential use of 
/// long arcs incurs a performance cost which you are likely not willing to 
/// pay when not using these long arcs.
#[derive(Debug, Clone)]
pub struct WithLongArcs<T>
where
    T: Eq + PartialEq + Hash + Clone,
{
    /// Keeps track of the decisions that have been taken to reach the root
    /// of this DD, starting from the problem root.
    root_pa: Vec<Decision>,
    /// All the nodes composing this decision diagram. The vector comprises 
    /// nodes from all layers in the DD. A nice property is that all nodes
    /// belonging to one same layer form a sequence in the ‘nodes‘ vector.
    nodes: Vec<Node<T>>,
    /// This vector stores the information about all edges connecting the nodes 
    /// of the decision diagram.
    edges: Vec<Edge>,
    /// The nodes from the next layer; those are the result of an application 
    /// of the transition function to a node in ‘prev_l‘.
    /// Note: next_l in itself is indexed on the state associated with nodes.
    /// The rationale being that two transitions to the same state in the same
    /// layer should lead to the same node. This indexation helps ensuring 
    /// the uniqueness constraint in amortized O(1).
    next_l: FxHashMap<Arc<T>, NodeId>,
    /// The identifiers of the nodes in the previous layer
    prev_l: Vec<NodeId>,
    /// The last exact layer of the decision diagram
    lel: Option<Vec<NodeId>>,
    /// The identifier of the best terminal node of the diagram (None when the
    /// problem compiled into this dd is infeasible)
    best_n: Option<NodeId>,
    /// A flag set to true when the longest r-t path of this decision diagram
    /// traverses no merged node (Exact Best Path Optimization aka EBPO).
    exact: bool,
}
impl<T> Default for WithLongArcs<T>
where
    T: Eq + PartialEq + Hash + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}
impl<T> DecisionDiagram for WithLongArcs<T>
where
    T: Eq + PartialEq + Hash + Clone,
{
    type State = T;

    fn compile(&mut self, input: &CompilationInput<T>)
        -> Result<Completion, Reason> {
        self._compile(input)
    }

    fn is_exact(&self) -> bool {
        self.exact
    }

    fn best_value(&self) -> Option<isize> {
        self._best_value()
    }

    fn best_solution(&self) -> Option<Vec<Decision>> {
        self._best_solution()
    }

    fn drain_cutset<F>(&mut self, func: F)
    where
        F: FnMut(SubProblem<T>),
    {
        self._drain_cutset(func)
    }
}
impl<T> WithLongArcs<T>
where
    T: Eq + PartialEq + Hash + Clone,
{
    pub fn new() -> Self {
        Self {
            root_pa: vec![],
            nodes: vec![],
            edges: vec![],
            prev_l: vec![],
            next_l: Default::default(),
            lel: None,
            best_n: None,
            exact: true,
        }
    }
    fn clear(&mut self) {
        self.root_pa.clear();
        self.nodes.clear();
        self.edges.clear();
        self.prev_l.clear();
        self.next_l.clear();
        self.lel = None;
        self.exact = true;
    }

    fn _is_exact(&self, comp_type: CompilationType) -> bool {
        self.lel.is_none()
            || (comp_type == CompilationType::Relaxed && self.has_exact_best_path(self.best_n))
    }

    fn has_exact_best_path(&self, node: Option<NodeId>) -> bool {
        if let Some(node_id) = node {
            let n = &self.nodes[node_id.0];
            if n.flags.is_exact() {
                true
            } else {
                !n.flags.is_relaxed()
                    && self.has_exact_best_path(n.best.map(|e| self.edges[e.0].from))
            }
        } else {
            true
        }
    }

    fn _best_value(&self) -> Option<isize> {
        self.best_n.map(|id| self.nodes[id.0].value)
    }

    fn _best_solution(&self) -> Option<Vec<Decision>> {
        self.best_n.map(|id| self._best_path(id))
    }

    fn _best_path(&self, id: NodeId) -> Vec<Decision> {
        Self::_best_path_partial_borrow(id, &self.root_pa, &self.nodes, &self.edges)
    }

    fn _best_path_partial_borrow(
        id: NodeId,
        root_pa: &[Decision],
        nodes: &[Node<T>],
        edges: &[Edge],
    ) -> Vec<Decision> {
        let mut sol = root_pa.to_owned();
        let mut edge_id = nodes[id.0].best;
        while let Some(eid) = edge_id {
            let edge = edges[eid.0];
            sol.push(edge.decision);
            edge_id = nodes[edge.from.0].best;
        }
        sol
    }

    fn _drain_cutset<F>(&mut self, mut func: F)
    where
        F: FnMut(SubProblem<T>),
    {
        if let Some(best_value) = self.best_value() {
            if let Some(lel) = self.lel.as_mut() {
                for id in lel.drain(..) {
                    let node = &self.nodes[id.0];

                    if node.flags.is_marked() {
                        let rub = node.value.saturating_add(node.rub);
                        let locb = node.value.saturating_add(node.value_bot);
                        let ub = rub.min(locb).min(best_value);

                        func(SubProblem {
                            state: node.state.clone(),
                            value: node.value,
                            path: Self::_best_path_partial_borrow(
                                id,
                                &self.root_pa,
                                &self.nodes,
                                &self.edges,
                            ),
                            ub,
                        })
                    }
                }
            }
        }
    }

    fn _compile(&mut self, input: &CompilationInput<T>)
        -> Result<Completion, Reason> {
        self.clear();

        let mut depth = 0;
        let mut curr_l = vec![];
        let mut long_arc = vec![];
        
        let root_s = Arc::new(input.residual.state.deref().clone());
        let root_v = input.residual.value;
        let root_n = Node {
            state: Arc::clone(&root_s),
            value: root_v,
            best: None,
            inbound: None,
            value_bot: isize::MIN,
            rub: input.residual.ub - root_v,
            flags: NodeFlags::new_exact(),
        };
        input
            .residual
            .path
            .iter()
            .copied()
            .for_each(|x| self.root_pa.push(x));

        self.nodes.push(root_n);
        self.next_l.insert(root_s, NodeId(0));

        while let Some(var) = input.problem.next_variable(&mut self.next_l.keys().map(|x| x.as_ref())) {
            // Did the cutoff kick in ?
            if input.cutoff.must_stop() {
                return Err(Reason::CutoffOccurred);
            }

            self.prev_l.clear();
            for (_, id) in curr_l.drain(..) {
                self.prev_l.push(id);
            }

            for (state, id) in self.next_l.drain() {
                if input.problem.is_impacted_by(var, &state) {
                    curr_l.push((state, id));
                } else {
                    long_arc.push((state, id));
                }
            }

            if curr_l.is_empty() && long_arc.is_empty() {
                break; 
            }

            match input.comp_type {
                CompilationType::Exact => { /* do nothing: you want to explore the complete DD */ }
                CompilationType::Restricted => {
                    if curr_l.len() > input.max_width {
                        self.maybe_save_lel();
                        self.restrict(input, &mut curr_l)
                    }
                }
                CompilationType::Relaxed => {
                    if curr_l.len() > input.max_width && depth > 1 {
                        let was_lel = self.maybe_save_lel();
                        //
                        if was_lel {
                            for (s, id) in curr_l.iter() {
                                let rub = input.relaxation.fast_upper_bound(s);
                                self.nodes[id.0].rub = rub;
                            }
                        }
                        //
                        self.relax(input, &mut curr_l)
                    }
                }
            }

            for tuple_s_id in long_arc.drain(..) {
                curr_l.push(tuple_s_id);
            }
            for (state, node_id) in curr_l.iter() {
                let rub = input.relaxation.fast_upper_bound(state);
                self.nodes[node_id.0].rub = rub;
                let ub = rub.saturating_add(self.nodes[node_id.0].value);
                if ub > input.best_lb {
                    input.problem.for_each_in_domain(var, state, &mut |decision| {
                        self.branch_on(state, *node_id, decision, input.problem)
                    })
                }
            }


            depth += 1;
        }

        //
        self.best_n = self
            .next_l
            .values()
            .copied()
            .max_by_key(|id| self.nodes[id.0].value);
        self.exact = self._is_exact(input.comp_type);
        //
        if matches!(input.comp_type, CompilationType::Relaxed) {
            self.compute_local_bounds();
        }

        Ok(Completion { is_exact: self.is_exact(), best_value: self.best_value() })
    }

    fn maybe_save_lel(&mut self) -> bool {
        if self.lel.is_none() {
            let mut lel = vec![];
            for id in self.prev_l.iter() {
                lel.push(*id);
                self.nodes[id.0].flags.set_cutset(true);
            }
            self.lel = Some(lel);
            true
        } else {
            false
        }
    }

    fn branch_on(
        &mut self,
        state: &T,
        from_id: NodeId,
        decision: Decision,
        problem: &dyn Problem<State = T>,
    ) {
        let next_state = Arc::new(problem.transition(state, decision));
        let cost = problem.transition_cost(state, decision);

        match self.next_l.entry(next_state.clone()) {
            Entry::Vacant(e) => {
                let node_id = NodeId(self.nodes.len());
                let edge_id = EdgeId(self.edges.len());

                self.edges.push(Edge {
                    //my_id: edge_id,
                    from: from_id,
                    //to   : node_id,
                    decision,
                    cost,
                    next: None,
                });
                self.nodes.push(Node {
                    state: next_state,
                    value: self.nodes[from_id.0].value.saturating_add(cost),
                    best: Some(edge_id),
                    inbound: Some(edge_id),
                    //
                    value_bot: isize::MIN,
                    //
                    rub: isize::MAX,
                    flags: self.nodes[from_id.0].flags,
                });

                e.insert(node_id);
            }
            Entry::Occupied(e) => {
                let node_id = *e.get();
                let exact = self.nodes[from_id.0].flags.is_exact();
                let value = self.nodes[from_id.0].value.saturating_add(cost);
                let node = &mut self.nodes[node_id.0];

                // flags hygiene
                let exact = exact & node.flags.is_exact();
                node.flags.set_exact(exact);

                let edge_id = EdgeId(self.edges.len());
                self.edges.push(Edge {
                    //my_id: edge_id,
                    from: from_id,
                    //to   : node_id,
                    decision,
                    cost,
                    next: node.inbound,
                });

                node.inbound = Some(edge_id);
                if value > node.value {
                    node.value = value;
                    node.best = Some(edge_id);
                }
            }
        }
    }

    fn restrict(&mut self, input: &CompilationInput<T>, curr_l: &mut Vec<(Arc<T>, NodeId)>) {
        curr_l.sort_unstable_by(|a, b| {
            self.nodes[a.1 .0]
                .value
                .cmp(&self.nodes[b.1 .0].value)
                .then_with(|| input.ranking.compare(a.0.as_ref(), b.0.as_ref()))
                .reverse()
        }); // reverse because greater means more likely to be kept
        curr_l.truncate(input.max_width);
    }

    fn relax(&mut self, input: &CompilationInput<T>, curr_l: &mut Vec<(Arc<T>, NodeId)>) {
        curr_l.sort_unstable_by(|a, b| {
            self.nodes[a.1 .0]
                .value
                .cmp(&self.nodes[b.1 .0].value)
                .then_with(|| input.ranking.compare(a.0.as_ref(), b.0.as_ref()))
                .reverse()
        }); // reverse because greater means more likely to be kept

        //--
        let (keep, merge) = curr_l.split_at_mut(input.max_width - 1);
        let merged = input.relaxation.merge(&mut merge.iter().map(|(k, _v)| k.as_ref()));
        let merged = Arc::new(merged);
        let recycled = keep.iter().find(|(k, _v)| k.eq(&merged)).map(|(_k, v)| *v);

        let merged_id = recycled.unwrap_or_else(|| {
            let node_id = NodeId(self.nodes.len());
            self.nodes.push(Node {
                state: Arc::clone(&merged),
                //my_id  : node_id,
                value: isize::MIN,
                best: None,    // yet
                inbound: None, // yet
                //
                value_bot: isize::MIN,
                //
                rub: isize::MAX,
                flags: NodeFlags::new_relaxed(),
            });
            node_id
        });

        self.nodes[merged_id.0].flags.set_relaxed(true);

        for (drop_k, drop_v) in merge {
            let mut edge_id = self.nodes[drop_v.0].inbound;
            while let Some(eid) = edge_id {
                let edge = self.edges[eid.0];
                let src = self.nodes[edge.from.0].state.as_ref();

                let rcost = input
                    .relaxation
                    .relax(src, drop_k, &merged, edge.decision, edge.cost);

                let new_eid = EdgeId(self.edges.len());
                let new_edge = Edge {
                    //my_id: new_eid,
                    from: edge.from,
                    //to   : merged_id,
                    decision: edge.decision,
                    cost: rcost,
                    next: self.nodes[merged_id.0].inbound,
                };
                self.edges.push(new_edge);
                self.nodes[merged_id.0].inbound = Some(new_eid);

                let new_value = self.nodes[edge.from.0].value.saturating_add(rcost);
                if new_value >= self.nodes[merged_id.0].value {
                    self.nodes[merged_id.0].best = Some(new_eid);
                    self.nodes[merged_id.0].value = new_value;
                }

                edge_id = edge.next;
            }
        }

        if recycled.is_some() {
            curr_l.truncate(input.max_width);
        } else {
            curr_l.truncate(input.max_width - 1);
            curr_l.push((merged, merged_id));
        }
    }

    fn compute_local_bounds(&mut self) {
        if !self.exact {
            // if it's exact, there is nothing to be done
            let mut visit = vec![];
            let mut next_v = vec![];

            // all the nodes from the last layer have a lp_from_bot of 0
            for id in self.next_l.values().copied() {
                self.nodes[id.0].value_bot = 0;
                self.nodes[id.0].flags.set_marked(true);
                visit.push(id);
            }

            while !visit.is_empty() {
                std::mem::swap(&mut visit, &mut next_v);

                for id in next_v.drain(..) {
                    let mut inbound = self.nodes[id.0].inbound;
                    while let Some(edge_id) = inbound {
                        let edge = self.edges[edge_id.0];

                        let lp_from_bot_using_edge =
                            self.nodes[id.0].value_bot.saturating_add(edge.cost);

                        self.nodes[edge.from.0].value_bot = self.nodes[edge.from.0]
                            .value_bot
                            .max(lp_from_bot_using_edge);

                        if !self.nodes[edge.from.0].flags.is_marked() {
                            self.nodes[edge.from.0].flags.set_marked(true);
                            if !self.nodes[edge.from.0].flags.is_cutset() {
                                visit.push(edge.from);
                            }
                        }

                        inbound = edge.next;
                    }
                }
            }
        }
    }
}




// ############################################################################
// #### TESTS #################################################################
// ############################################################################


#[cfg(test)]
mod test_default_mdd {
    use std::cmp::Ordering;
    use std::sync::Arc;

    use rustc_hash::FxHashMap;

    use crate::{Variable, WithLongArcs, DecisionDiagram, SubProblem, CompilationInput, Problem, Decision, Relaxation, StateRanking, NoCutoff, CompilationType, Cutoff, Reason, DecisionCallback};

    #[test]
    fn by_default_the_mdd_type_is_exact() {
        let mdd = WithLongArcs::<usize>::new();

        assert!(mdd.is_exact());
    }

    #[test]
    fn root_remembers_the_pa_from_the_frontier_node() {
        let mut input = CompilationInput {
            comp_type: crate::CompilationType::Exact,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  3,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 1, value: 42}), 
                value: 42, 
                path:  vec![Decision{variable: Variable(0), value: 42}], 
                ub:    isize::MAX
            }
        };

        let mut mdd = WithLongArcs::new();
        assert!(mdd.compile(&input).is_ok());
        assert_eq!(mdd.root_pa, vec![Decision{variable: Variable(0), value: 42}]);

        input.comp_type = CompilationType::Relaxed;
        assert!(mdd.compile(&input).is_ok());
        assert_eq!(mdd.root_pa, vec![Decision{variable: Variable(0), value: 42}]);

        input.comp_type = CompilationType::Restricted;
        assert!(mdd.compile(&input).is_ok());
        assert_eq!(mdd.root_pa, vec![Decision{variable: Variable(0), value: 42}]);
    }
    
    // In an exact setup, the dummy problem would be 3*3*3 = 9 large at the bottom level
    #[test]
    fn exact_completely_unrolls_the_mdd_no_matter_its_width() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Exact,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();

        assert!(mdd.compile(&input).is_ok());
        assert!(mdd.best_solution().is_some());
        assert_eq!(mdd.best_value(), Some(6));
        assert_eq!(mdd.best_solution().unwrap(),
                   vec![
                       Decision{variable: Variable(2), value: 2},
                       Decision{variable: Variable(1), value: 2},
                       Decision{variable: Variable(0), value: 2},
                   ]
        );
    }

    #[test]
    fn restricted_drops_the_less_interesting_nodes() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Restricted,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();

        assert!(mdd.compile(&input).is_ok());
        assert!(mdd.best_solution().is_some());
        assert_eq!(mdd.best_value().unwrap(), 6);
        assert_eq!(mdd.best_solution().unwrap(),
                   vec![
                       Decision{variable: Variable(2), value: 2},
                       Decision{variable: Variable(1), value: 2},
                       Decision{variable: Variable(0), value: 2},
                   ]
        );
    }

    #[test]
    fn exact_no_cutoff_completion_must_be_coherent_with_outcome() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Exact,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);

        assert!(result.is_ok());
        let completion = result.unwrap();
        assert_eq!(completion.is_exact  , mdd.is_exact());
        assert_eq!(completion.best_value, mdd.best_value());
    }
    #[test]
    fn restricted_no_cutoff_completion_must_be_coherent_with_outcome_() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Restricted,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        
        assert!(result.is_ok());
        let completion = result.unwrap();
        assert_eq!(completion.is_exact  , mdd.is_exact());
        assert_eq!(completion.best_value, mdd.best_value());
    }
    #[test]
    fn relaxed_no_cutoff_completion_must_be_coherent_with_outcome() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        
        assert!(result.is_ok());
        let completion = result.unwrap();
        assert_eq!(completion.is_exact  , mdd.is_exact());
        assert_eq!(completion.best_value, mdd.best_value());
    }
    
    #[derive(Debug, Clone, Copy)]
    struct CutoffAlways;
    impl Cutoff for CutoffAlways {
        fn must_stop(&self) -> bool { true }
    }
    #[test]
    fn exact_fails_with_cutoff_when_cutoff_occurs() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Exact,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &CutoffAlways,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_err());
        assert_eq!(Some(Reason::CutoffOccurred), result.err());
    }

    #[test]
    fn restricted_fails_with_cutoff_when_cutoff_occurs() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Restricted,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &CutoffAlways,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_err());
        assert_eq!(Some(Reason::CutoffOccurred), result.err());
    }
    #[test]
    fn relaxed_fails_with_cutoff_when_cutoff_occurs() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &CutoffAlways,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_err());
        assert_eq!(Some(Reason::CutoffOccurred), result.err());
    }

    #[test]
    fn relaxed_merges_the_less_interesting_nodes() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);

        assert!(result.is_ok());
        assert!(mdd.best_solution().is_some());
        assert_eq!(mdd.best_value().unwrap(), 24);
        assert_eq!(mdd.best_solution().unwrap(),
                   vec![
                       Decision{variable: Variable(2), value: 2},
                       Decision{variable: Variable(1), value: 0}, // that's a relaxed edge
                       Decision{variable: Variable(0), value: 2},
                   ]
        );
    }

    #[test]
    fn relaxed_populates_the_cutset_and_will_not_squash_first_layer() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        
        let mut cutset = vec![];
        mdd.drain_cutset(|n| cutset.push(n));
        assert_eq!(cutset.len(), 3); // L1 was not squashed even though it was 3 wide
    }

    #[test]
    fn an_exact_mdd_must_be_exact() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Exact,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        
        assert_eq!(true, mdd.is_exact())
    }

    #[test]
    fn a_relaxed_mdd_is_exact_as_long_as_no_merge_occurs() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  10,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        
        assert_eq!(true, mdd.is_exact())
    }

    #[test]
    fn a_relaxed_mdd_is_not_exact_when_a_merge_occured() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        
        assert_eq!(false, mdd.is_exact())
    }
    #[test]
    fn a_restricted_mdd_is_exact_as_long_as_no_restriction_occurs() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Restricted,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  10,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        
        assert_eq!(true, mdd.is_exact())
    }
    #[test]
    fn a_restricted_mdd_is_not_exact_when_a_restriction_occured() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  1,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        
        assert_eq!(false, mdd.is_exact())
    }
    #[test]
    fn when_the_problem_is_infeasible_there_is_no_solution() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Exact,
            problem:    &DummyInfeasibleProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  usize::MAX,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        assert!(mdd.best_solution().is_none())
    }
    #[test]
    fn when_the_problem_is_infeasible_there_is_no_best_value() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Exact,
            problem:    &DummyInfeasibleProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  usize::MAX,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        assert!(mdd.best_value().is_none())
    }
    #[test]
    fn exact_skips_node_with_an_ub_less_than_best_known_lb() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Exact,
            problem:    &DummyInfeasibleProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  usize::MAX,
            best_lb:    1000,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        assert!(mdd.best_solution().is_none())
    }
    #[test]
    fn relaxed_skips_node_with_an_ub_less_than_best_known_lb() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyInfeasibleProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  usize::MAX,
            best_lb:    1000,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        assert!(mdd.best_solution().is_none())
    }
    #[test]
    fn restricted_skips_node_with_an_ub_less_than_best_known_lb() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Restricted,
            problem:    &DummyInfeasibleProblem,
            relaxation: &DummyRelax,
            ranking:    &DummyRanking,
            cutoff:     &NoCutoff,
            max_width:  usize::MAX,
            best_lb:    1000,
            residual: SubProblem { 
                state: Arc::new(DummyState{depth: 0, value: 0}), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        assert!(mdd.best_solution().is_none())
    }

    #[test]
    fn it_must_be_possible_to_introduce_long_arcs() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Restricted,
            problem:    &DummyLongArcProblem,
            relaxation: &DummyLongArcRelax,
            ranking:    &DummyLongArcRanking,
            cutoff:     &NoCutoff,
            max_width:  usize::MAX,
            best_lb:    1000,
            residual: SubProblem { 
                state: Arc::new('e'), 
                value: 1, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        assert!(mdd.best_solution().is_some());
        assert!(mdd.best_solution().unwrap().is_empty());
    }

    #[test]
    fn exact_cutset_must_include_long_arcs() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &DummyLongArcProblem,
            relaxation: &DummyLongArcRelax,
            ranking:    &DummyLongArcRanking,
            cutoff:     &NoCutoff,
            max_width:  2,
            best_lb:    isize::MIN,
            residual: SubProblem { 
                state: Arc::new('a'), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());
        assert!(mdd.best_solution().is_some());
        
        let mut cutset = vec![];
        mdd.drain_cutset(|x| {
            cutset.push(*x.state.as_ref())
        });

        cutset.sort();
        assert_eq!(vec!['c', 'd', 'e'], cutset);
    }

    /// The example problem and relaxation for the local bounds should generate
    /// the following relaxed MDD in which the layer 'a','b' is the LEL.
    ///
    /// ```plain
    ///                      r
    ///                   /     \
    ///                10        7
    ///               /           |
    ///             a              b
    ///             |     +--------+-------+
    ///             |     |        |       |
    ///             2     3        6       5
    ///              \   /         |       |
    ///                M           e       f
    ///                |           |     /   \
    ///                4           0   1      2
    ///                |           |  /        \
    ///                g            h           i
    ///                |            |           |
    ///                0            0           0
    ///                +------------+-----------+
    ///                             t
    /// ```
    ///
    #[derive(Copy, Clone)]
    struct LocBoundsExamplePb;
    impl Problem for LocBoundsExamplePb {
        type State = char;
        fn nb_variables (&self) -> usize {  3  }
        fn initial_state(&self) -> char  { 'r' }
        fn initial_value(&self) -> isize {  0  }
        fn next_variable(&self, next_layer: &mut dyn Iterator<Item = &Self::State>) -> Option<Variable> {
            match next_layer.next().copied().unwrap_or('z') {
                'r' => Some(Variable(0)),
                'a' => Some(Variable(1)),
                'b' => Some(Variable(1)),
                // c, d are merged into M
                'c' => Some(Variable(2)),
                'd' => Some(Variable(2)),
                'M' => Some(Variable(2)),
                'e' => Some(Variable(2)),
                'f' => Some(Variable(2)),
                _   => None,
            }
        }
        fn for_each_in_domain(&self, variable: Variable, state: &Self::State, f: &mut dyn DecisionCallback) {
            /* do nothing, just consider that all domains are empty */
            (match *state {
                'r' => vec![10, 7],
                'a' => vec![2],
                'b' => vec![3, 6, 5],
                // c, d are merged into M
                'M' => vec![4],
                'e' => vec![0],
                'f' => vec![1, 2],
                _   => vec![],
            })
            .iter()
            .copied()
            .for_each(&mut |value| f.apply(Decision{variable, value}))
        }

        fn transition(&self, state: &char, d: Decision) -> char {
            match (*state, d.value) {
                ('r', 10) => 'a',
                ('r',  7) => 'b',
                ('a',  2) => 'c', // merged into M
                ('b',  3) => 'd', // merged into M
                ('b',  6) => 'e',
                ('b',  5) => 'f',
                ('M',  4) => 'g',
                ('e',  0) => 'h',
                ('f',  1) => 'h',
                ('f',  2) => 'i',
                _         => 't'
            }
        }

        fn transition_cost(&self, _: &char, d: Decision) -> isize {
            d.value
        }
    }

    #[derive(Copy, Clone)]
    struct LocBoundExampleRelax;
    impl Relaxation for LocBoundExampleRelax {
        type State = char;
        fn merge(&self, _: &mut dyn Iterator<Item=&char>) -> char {
            'M'
        }

        fn relax(&self, _: &char, _: &char, _: &char, _: Decision, cost: isize) -> isize {
            cost
        }
    }

    #[derive(Clone, Copy)]
    struct CmpChar;
    impl StateRanking for CmpChar {
        type State = char;
        fn compare(&self, a: &char, b: &char) -> Ordering {
            a.cmp(b)
        }
    }

    #[test]
    fn relaxed_computes_local_bounds() {
        let input = CompilationInput {
            comp_type: crate::CompilationType::Relaxed,
            problem:    &LocBoundsExamplePb,
            relaxation: &LocBoundExampleRelax,
            ranking:    &CmpChar,
            cutoff:     &NoCutoff,
            max_width:  3,
            best_lb:    0,
            residual: SubProblem { 
                state: Arc::new('r'), 
                value: 0, 
                path:  vec![], 
                ub:    isize::MAX
            }
        };
        let mut mdd = WithLongArcs::new();
        let result = mdd.compile(&input);
        assert!(result.is_ok());

        assert_eq!(false,    mdd.is_exact());
        assert_eq!(Some(16), mdd.best_value());

        let mut v = FxHashMap::<char, isize>::default();
        mdd.drain_cutset(|n| {v.insert(*n.state, n.ub);});

        assert_eq!(16, v[&'a']);
        assert_eq!(14, v[&'b']);
    }

    #[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
    struct DummyState {
        value: isize,
        depth: usize,
    }

    #[derive(Copy, Clone)]
    struct DummyProblem;
    impl Problem for DummyProblem {
        type State = DummyState;

        fn nb_variables(&self)  -> usize { 3 }
        fn initial_value(&self) -> isize { 0 }
        fn initial_state(&self) -> Self::State {
            DummyState {
                value: 0,
                depth: 0,
            }
        }

        fn transition(&self, state: &Self::State, decision: crate::Decision) -> Self::State {
            DummyState {
                value: state.value + decision.value,
                depth: 1 + state.depth
            }
        }

        fn transition_cost(&self, _: &Self::State, decision: crate::Decision) -> isize {
            decision.value
        }

        fn next_variable(&self, next_layer: &mut dyn Iterator<Item = &Self::State>)
            -> Option<crate::Variable> {
            next_layer.next()
                .map(|x| x.depth)
                .filter(|d| *d < self.nb_variables())
                .map(Variable)
        }

        fn for_each_in_domain(&self, var: crate::Variable, _: &Self::State, f: &mut dyn DecisionCallback) {
            for d in 0..=2 {
                f.apply(Decision {variable: var, value: d})
            }
        }
    }

    #[derive(Clone,Copy)]
    struct DummyInfeasibleProblem;
    impl Problem for DummyInfeasibleProblem {
        type State = DummyState;

        fn nb_variables(&self)  -> usize { 3 }
        fn initial_value(&self) -> isize { 0 }
        fn initial_state(&self) -> Self::State {
            DummyState {
                value: 0,
                depth: 0,
            }
        }

        fn transition(&self, state: &Self::State, decision: crate::Decision) -> Self::State {
            DummyState {
                value: state.value + decision.value,
                depth: 1 + state.depth
            }
        }

        fn transition_cost(&self, _: &Self::State, decision: crate::Decision) -> isize {
            decision.value
        }

        fn next_variable(&self, next_layer: &mut dyn Iterator<Item = &Self::State>)
            -> Option<crate::Variable> {
            next_layer.next()
                .map(|x| x.depth)
                .filter(|d| *d < self.nb_variables())
                .map(Variable)
        }

        fn for_each_in_domain(&self, _: crate::Variable, _: &Self::State, _: &mut dyn DecisionCallback) {
            /* do nothing, just consider that all domains are empty */
        }
    }

    #[derive(Copy, Clone)]
    struct DummyLongArcProblem;
    impl Problem for DummyLongArcProblem {
        type State = char;

        fn nb_variables(&self)  -> usize { 5 }
        fn initial_value(&self) -> isize { 0 }
        fn initial_state(&self) -> char  {'a'}

        fn transition(&self, s: &Self::State, d: crate::Decision) -> Self::State {
            let Decision{variable, value, ..} = d;
            match (*s, variable.id(), value) {
                ('a', 0, _)=> 'b',

                ('b', 1, _)=> 'b',
                
                ('b', 2, 0)=> 'c',
                ('b', 2, 1)=> 'd',
                ('b', 2, 2)=> 'e',
                
                ('c', 3, 0)=> 'f',
                ('c', 3, 1)=> 'g',
                ('c', 3, 2)=> 'h',
                ('d', 3, 0)=> 'i',
                ('d', 3, 1)=> 'j',
                ('d', 3, 2)=> 'k',

                _ => 'x',
                
            }
        }

        fn transition_cost(&self, state: &Self::State, _: crate::Decision) -> isize {
            match *state {
                'a' => 1,
                'b' => 2,
                'c' => 3,
                'd' => 1,
                'e' => 1,
                'f' => 1,
                'g' => 1,
                'h' => 1,
                'i' => 1,
                'j' => 1,
                'k' => 1,
                'M' => 100000,
                _   => 1
            }
        }

        fn next_variable(&self, _: &mut dyn Iterator<Item = &Self::State>)
            -> Option<crate::Variable> {
            
            static mut COUNT : usize = 0;
            let value = unsafe {
                let x = COUNT;
                COUNT +=1;
                x
            };

            if value < self.nb_variables() { Some(Variable(value))} else {None}
        }

        fn for_each_in_domain(&self, var: crate::Variable, _: &Self::State, f: &mut dyn DecisionCallback) {
            for d in 0..=2 {
                f.apply(Decision {variable: var, value: d})
            }
        }

        fn is_impacted_by(&self, _: Variable, state: &Self::State) -> bool {
            *state != 'e'
        }
    }

    #[derive(Copy, Clone)]
    struct DummyLongArcRelax;
    impl Relaxation for DummyLongArcRelax {
        type State = char;

        fn merge(&self, _it: &mut dyn Iterator<Item = &Self::State>) -> Self::State {
            'M'
        }

        fn relax(
            &self,
            _: &Self::State,
            _: &Self::State,
            _: &Self::State,
            _: Decision,
            cost: isize,
        ) -> isize {
            cost
        }
    }
    #[derive(Copy, Clone)]
    struct DummyLongArcRanking;
    impl StateRanking for DummyLongArcRanking {
        type State = char;

        fn compare(&self, a: &Self::State, b: &Self::State) -> Ordering {
            a.cmp(b)
        }
    }

    #[derive(Copy, Clone)]
    struct DummyRelax;
    impl Relaxation for DummyRelax {
        type State = DummyState;

        fn merge(&self, s: &mut dyn Iterator<Item=&Self::State>) -> Self::State {
            s.next().map(|s| {
                DummyState {
                    value: 100,
                    depth: s.depth
                }
            }).unwrap()
        }
        fn relax(&self, _: &Self::State, _: &Self::State, _: &Self::State, _: Decision, _: isize) -> isize {
            20
        }
        fn fast_upper_bound(&self, _state: &Self::State) -> isize {
            50
        }
    }

    #[derive(Copy, Clone)]
    struct DummyRanking;
    impl StateRanking for DummyRanking {
        type State = DummyState;

        fn compare(&self, a: &Self::State, b: &Self::State) -> Ordering {
            a.value.cmp(&b.value).reverse()
        }
    }
}