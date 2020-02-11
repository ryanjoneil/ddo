#![cfg(test)]
extern crate rust_mdd_solver;

use std::fs::File;
use std::path::PathBuf;

use rust_mdd_solver::core::abstraction::solver::Solver;
use rust_mdd_solver::core::implementation::bb_solver::BBSolver;
use rust_mdd_solver::core::implementation::heuristics::{MaxUB, FromLongestPath, FixedWidth};
use rust_mdd_solver::examples::max2sat::heuristics::{Max2SatOrder, MinRank};
use rust_mdd_solver::examples::max2sat::model::Max2Sat;
use rust_mdd_solver::examples::max2sat::relax::Max2SatRelax;
use rust_mdd_solver::core::implementation::mdd::flat::FlatMDD;

/// This method simply loads a resource into a problem instance to solve
fn instance(id: &str) -> Max2Sat {
    let location = PathBuf::new()
        .join(env!("CARGO_MANIFEST_DIR"))
        .join("tests/resources/max2sat/")
        .join(id);

    File::open(location).expect("File not found").into()
}

/// This is the function we will use to actually solve an instance to completion
/// and check that the optimal value it identifies corresponds to the expected
/// value.
fn solve(id: &str) -> i32 {
    let problem      = instance(id);
    let relax        = Max2SatRelax::new(&problem);
    let width        = FixedWidth(5);
    let vs           = Max2SatOrder::new(&problem);
    let ns           = MinRank;
    let bo           = MaxUB;
    let vars         = FromLongestPath::new(&problem);

    let ddg          = FlatMDD::new(&problem, relax, vs, width, ns);
    let mut solver   = BBSolver::new(ddg, bo, vars);
    //solver.verbosity = 3;
    let (val,_sln)   = solver.maximize();
    val
}

#[test]
fn debug() {
    assert_eq!(solve("debug.wcnf"), 24);
}
#[test]
fn debug2() {
    assert_eq!(solve("debug2.wcnf"), 13);
}

#[test]
fn pass() {
    assert_eq!(solve("pass.wcnf"), 54);
}
#[test]
fn tautology() {
    assert_eq!(solve("tautology.wcnf"), 7);
}
#[test]
fn unit() {
    assert_eq!(solve("unit.wcnf"), 6);
}
#[test]
fn negative_wt() {
    assert_eq!(solve("negative_wt.wcnf"), 4258);
}

#[test]
fn frb10_6_1() {
    assert_eq!(solve("frb10-6-1.wcnf"), 37_037);
}
#[test]
fn frb10_6_2() {
    assert_eq!(solve("frb10-6-2.wcnf"), 38_196);
}
#[test]
fn frb10_6_3() {
    assert_eq!(solve("frb10-6-3.wcnf"), 36_671);
}
#[test]
fn frb10_6_4() {
    assert_eq!(solve("frb10-6-4.wcnf"), 38_928);
}
#[ignore] #[test]
fn frb15_9_1() {
    assert_eq!(solve("frb15-9-1.wcnf"), 341_783);
}
#[ignore] #[test]
fn frb15_9_2() {
    assert_eq!(solve("frb15-9-2.wcnf"), 341_919);
}
#[ignore] #[test]
fn frb15_9_3() {
    assert_eq!(solve("frb15-9-3.wcnf"), 339_471);
}
#[ignore] #[test]
fn frb15_9_4() {
    assert_eq!(solve("frb15-9-4.wcnf"), 340_559);
}
#[ignore] #[test]
fn frb15_9_5() {
    assert_eq!(solve("frb15-9-5.wcnf"), 348_311);
}
#[ignore] #[test]
fn frb20_11_1() {
    assert_eq!(solve("frb20-11-1.wcnf"), 1_245_134);
}
#[ignore] #[test]
fn frb20_11_2() {
    assert_eq!(solve("frb20-11-2.wcnf"), 1_231_874);
}
#[ignore] #[test]
fn frb20_11_3() {
    assert_eq!(solve("frb20-11-2.wcnf"), 1_240_493);
}
#[ignore] #[test]
fn frb20_11_4() {
    assert_eq!(solve("frb20-11-4.wcnf"), 1_231_653);
}
#[ignore] #[test]
fn frb20_11_5() {
    assert_eq!(solve("frb20-11-5.wcnf"), 1_237_841);
}