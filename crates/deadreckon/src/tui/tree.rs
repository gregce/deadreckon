#![allow(dead_code)]

use std::path::PathBuf;

use deadreckon_core::campaign::{Campaign, CampaignEvent, CampaignStatus, SubGoal, SubGoalStatus};
use deadreckon_core::state::{load_state, spend_summary};
use deadreckon_core::{
    Chain, ChainEvent, ChainEventKind, ChainStatus, ChainStep, ChainStepStatus, DeadreckonPaths,
    PipelineState, Plan, PlanEvent, PlanEventKind, PlanStatus, PlanTask, PlanTaskStatus, RunStatus,
    load_chain, load_plan, load_run,
};
use deadreckon_protocol::{RunEvent, RunEventKind};

use crate::commands::campaign::CampaignFeedEvent;
use crate::plan_event_bus::PlanFeedEvent;

use super::super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttachTarget {
    Run {
        state_path: PathBuf,
    },
    Plan {
        paths: DeadreckonPaths,
        plan_id: String,
    },
    Chain {
        paths: DeadreckonPaths,
        chain_id: String,
    },
    Campaign {
        paths: DeadreckonPaths,
        campaign_dir: PathBuf,
        campaign_id: String,
    },
}

impl AttachTarget {
    pub(crate) fn run(state: &PipelineState) -> Self {
        Self::Run {
            state_path: state.state_path(),
        }
    }

    pub(crate) fn plan(paths: &DeadreckonPaths, plan_id: &str) -> Self {
        Self::Plan {
            paths: paths.clone(),
            plan_id: plan_id.to_string(),
        }
    }

    pub(crate) fn chain(paths: &DeadreckonPaths, chain_id: &str) -> Self {
        Self::Chain {
            paths: paths.clone(),
            chain_id: chain_id.to_string(),
        }
    }

    pub(crate) fn campaign(
        paths: &DeadreckonPaths,
        campaign_dir: impl Into<PathBuf>,
        campaign_id: &str,
    ) -> Self {
        Self::Campaign {
            paths: paths.clone(),
            campaign_dir: campaign_dir.into(),
            campaign_id: campaign_id.to_string(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NodeId {
    Run(String),
    Plan(String),
    Task { plan_id: String, task_id: String },
    Chain(String),
    ChainStep { chain_id: String, step_index: u32 },
    Campaign(String),
    SubGoal { campaign_id: String, sub_id: String },
}

impl NodeId {
    pub(crate) fn run(run_id: &str) -> Self {
        Self::Run(run_id.to_string())
    }

    pub(crate) fn plan(plan_id: &str) -> Self {
        Self::Plan(plan_id.to_string())
    }

    pub(crate) fn task(plan_id: &str, task_id: &str) -> Self {
        Self::Task {
            plan_id: plan_id.to_string(),
            task_id: task_id.to_string(),
        }
    }

    pub(crate) fn chain(chain_id: &str) -> Self {
        Self::Chain(chain_id.to_string())
    }

    pub(crate) fn chain_step(chain_id: &str, step_index: u32) -> Self {
        Self::ChainStep {
            chain_id: chain_id.to_string(),
            step_index,
        }
    }

    pub(crate) fn campaign(campaign_id: &str) -> Self {
        Self::Campaign(campaign_id.to_string())
    }

    pub(crate) fn sub_goal(campaign_id: &str, sub_id: &str) -> Self {
        Self::SubGoal {
            campaign_id: campaign_id.to_string(),
            sub_id: sub_id.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeKind {
    Run,
    Plan,
    Task,
    Chain,
    ChainStep,
    Campaign,
    SubGoal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeStatus {
    Pending,
    Running,
    Gated,
    Verified,
    Failed,
    Paused,
    Killed,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TreeNode {
    pub(crate) id: NodeId,
    pub(crate) kind: NodeKind,
    pub(crate) label: String,
    pub(crate) status: NodeStatus,
    pub(crate) gate: Option<(u32, u32)>,
    pub(crate) spend: Option<f64>,
    pub(crate) children: Vec<TreeNode>,
}

impl TreeNode {
    fn new(id: NodeId, kind: NodeKind, label: impl Into<String>, status: NodeStatus) -> Self {
        Self {
            id,
            kind,
            label: label.into(),
            status,
            gate: None,
            spend: None,
            children: Vec::new(),
        }
    }

    fn with_spend(mut self, spend: Option<f64>) -> Self {
        self.spend = spend;
        self
    }

    fn max_depth(&self) -> usize {
        self.children
            .iter()
            .map(TreeNode::max_depth)
            .max()
            .map_or(1, |depth| depth + 1)
    }

    fn find(&self, id: &NodeId) -> Option<&TreeNode> {
        if &self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }

    fn find_mut(&mut self, id: &NodeId) -> Option<&mut TreeNode> {
        if &self.id == id {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_mut(id))
    }

    fn find_task_mut(&mut self, task_id: &str) -> Option<&mut TreeNode> {
        match &self.id {
            NodeId::Task { task_id: id, .. } if id == task_id => return Some(self),
            _ => {}
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_task_mut(task_id))
    }

    fn push_or_replace_child(&mut self, node: TreeNode) {
        if let Some(existing) = self.children.iter_mut().find(|child| child.id == node.id) {
            *existing = node;
        } else {
            self.children.push(node);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TreeModel {
    pub(crate) root: TreeNode,
}

impl TreeModel {
    pub(crate) fn max_depth(&self) -> usize {
        self.root.max_depth()
    }

    pub(crate) fn find(&self, id: &NodeId) -> Option<&TreeNode> {
        self.root.find(id)
    }

    fn find_mut(&mut self, id: &NodeId) -> Option<&mut TreeNode> {
        self.root.find_mut(id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TreeEvent {
    Run(RunEvent),
    Plan(PlanFeedEvent),
    Chain(ChainEvent),
    Campaign(CampaignFeedEvent),
}

pub(crate) fn build_tree(target: AttachTarget) -> Result<TreeModel> {
    match target {
        AttachTarget::Run { state_path } => {
            let state = load_state(&state_path)?;
            Ok(tree_for_run(&state))
        }
        AttachTarget::Plan { paths, plan_id } => {
            let plan = load_plan(&paths, &plan_id)?;
            Ok(tree_for_plan(&paths, &plan))
        }
        AttachTarget::Chain { paths, chain_id } => {
            let chain = load_chain(&paths, &chain_id)?;
            Ok(tree_for_chain(&paths, &chain))
        }
        AttachTarget::Campaign {
            paths,
            campaign_dir,
            campaign_id,
        } => {
            let campaign = deadreckon_core::campaign::read_campaign(&campaign_dir)?;
            if campaign.campaign_id != campaign_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "campaign id changed on disk: expected {campaign_id}, found {}",
                    campaign.campaign_id
                ))));
            }
            Ok(tree_for_campaign(&paths, &campaign))
        }
    }
}

pub(crate) fn tree_for_run(state: &PipelineState) -> TreeModel {
    TreeModel {
        root: run_node(state),
    }
}

pub(crate) fn tree_for_plan(paths: &DeadreckonPaths, plan: &Plan) -> TreeModel {
    TreeModel {
        root: plan_node(paths, plan),
    }
}

pub(crate) fn tree_for_chain(paths: &DeadreckonPaths, chain: &Chain) -> TreeModel {
    TreeModel {
        root: chain_node(paths, chain),
    }
}

pub(crate) fn tree_for_campaign(paths: &DeadreckonPaths, campaign: &Campaign) -> TreeModel {
    TreeModel {
        root: campaign_node(paths, campaign),
    }
}

pub(crate) fn fold_events(model: &mut TreeModel, batch: &[TreeEvent]) {
    for event in batch {
        match event {
            TreeEvent::Run(event) => fold_run_event(model, event, None),
            TreeEvent::Plan(event) => fold_plan_feed_event(model, event),
            TreeEvent::Chain(event) => fold_chain_event(model, event),
            TreeEvent::Campaign(event) => fold_campaign_feed_event(model, event),
        }
    }
}

fn campaign_node(paths: &DeadreckonPaths, campaign: &Campaign) -> TreeNode {
    let mut node = TreeNode::new(
        NodeId::campaign(&campaign.campaign_id),
        NodeKind::Campaign,
        campaign.root_goal.clone(),
        campaign_status(campaign.status),
    )
    .with_spend(campaign.tree_budget_usd.map(|_| {
        campaign
            .sub_goals
            .iter()
            .filter_map(|sub| sub.result_run_id.as_deref())
            .filter_map(|run_id| load_run(paths, run_id).ok())
            .map(|state| run_spend(&state))
            .sum()
    }));
    for sub in &campaign.sub_goals {
        node.children.push(sub_goal_node(paths, campaign, sub));
    }
    node
}

fn sub_goal_node(paths: &DeadreckonPaths, campaign: &Campaign, sub: &SubGoal) -> TreeNode {
    let mut node = TreeNode::new(
        NodeId::sub_goal(&campaign.campaign_id, &sub.sub_id),
        NodeKind::SubGoal,
        sub.goal.clone(),
        sub_goal_status(sub.status),
    );
    if let Some(plan_id) = sub.sub_plan_id.as_deref()
        && let Ok(plan) = load_plan(paths, plan_id)
    {
        for task in &plan.tasks {
            node.children.push(task_node(paths, &plan, task));
        }
    }
    node
}

fn plan_node(paths: &DeadreckonPaths, plan: &Plan) -> TreeNode {
    let mut node = TreeNode::new(
        NodeId::plan(&plan.plan_id),
        NodeKind::Plan,
        plan.root_goal.clone(),
        plan_status(plan.status),
    )
    .with_spend(Some(plan_spend(paths, plan)));
    for task in &plan.tasks {
        node.children.push(task_node(paths, plan, task));
    }
    node
}

fn task_node(paths: &DeadreckonPaths, plan: &Plan, task: &PlanTask) -> TreeNode {
    let mut node = TreeNode::new(
        NodeId::task(&plan.plan_id, &task.task_id),
        NodeKind::Task,
        task_label(task),
        plan_task_status(task.status),
    );
    if let Some(run_id) = task.child_run_id.as_deref() {
        node.children.push(
            load_run(paths, run_id)
                .map(|state| run_node(&state))
                .unwrap_or_else(|_| placeholder_run_node(run_id, NodeStatus::Running)),
        );
    }
    node
}

fn chain_node(paths: &DeadreckonPaths, chain: &Chain) -> TreeNode {
    let mut node = TreeNode::new(
        NodeId::chain(&chain.chain_id),
        NodeKind::Chain,
        chain.root_goal.clone(),
        chain_status(chain.status),
    )
    .with_spend(Some(chain.total_spend_usd));
    for step in &chain.steps {
        node.children.push(chain_step_node(paths, chain, step));
    }
    node
}

fn chain_step_node(paths: &DeadreckonPaths, chain: &Chain, step: &ChainStep) -> TreeNode {
    let mut node = TreeNode::new(
        NodeId::chain_step(&chain.chain_id, step.index),
        NodeKind::ChainStep,
        step.goal.clone(),
        chain_step_status(step.status),
    )
    .with_spend(Some(step.spend_usd));
    if let Some(run_id) = step.run_id.as_deref() {
        node.children.push(
            load_run(paths, run_id)
                .map(|state| run_node(&state))
                .unwrap_or_else(|_| placeholder_run_node(run_id, chain_step_status(step.status))),
        );
    }
    node
}

fn run_node(state: &PipelineState) -> TreeNode {
    TreeNode::new(
        NodeId::run(&state.run_id),
        NodeKind::Run,
        state.goal.clone(),
        run_status(state),
    )
    .with_spend(Some(run_spend(state)))
}

fn placeholder_run_node(run_id: &str, status: NodeStatus) -> TreeNode {
    TreeNode::new(
        NodeId::run(run_id),
        NodeKind::Run,
        format!("run {}", run_prefix(run_id)),
        status,
    )
}

fn task_label(task: &PlanTask) -> String {
    if task.subject.trim().is_empty() {
        task.goal.clone()
    } else {
        task.subject.clone()
    }
}

fn run_spend(state: &PipelineState) -> f64 {
    spend_summary(state)
        .map(|summary| summary.total_usd)
        .unwrap_or(state.total_spend_usd)
}

fn plan_spend(paths: &DeadreckonPaths, plan: &Plan) -> f64 {
    plan.tasks
        .iter()
        .filter_map(|task| task.child_run_id.as_deref())
        .filter_map(|run_id| load_run(paths, run_id).ok())
        .map(|state| run_spend(&state))
        .sum()
}

fn run_status(state: &PipelineState) -> NodeStatus {
    if state.pause_reason.is_some()
        && matches!(
            state.status,
            RunStatus::Pending | RunStatus::Planned | RunStatus::Executing
        )
    {
        return NodeStatus::Paused;
    }
    match state.status {
        RunStatus::Pending | RunStatus::Planned => NodeStatus::Pending,
        RunStatus::Executing => NodeStatus::Running,
        RunStatus::Completed => NodeStatus::Verified,
        RunStatus::Failed => NodeStatus::Failed,
        RunStatus::Killed => NodeStatus::Killed,
    }
}

fn plan_status(status: PlanStatus) -> NodeStatus {
    match status {
        PlanStatus::Pending => NodeStatus::Pending,
        PlanStatus::Forked => NodeStatus::Running,
        PlanStatus::Merged => NodeStatus::Verified,
        PlanStatus::Failed => NodeStatus::Failed,
    }
}

fn plan_task_status(status: PlanTaskStatus) -> NodeStatus {
    match status {
        PlanTaskStatus::Pending => NodeStatus::Pending,
        PlanTaskStatus::Running => NodeStatus::Running,
        PlanTaskStatus::Completed => NodeStatus::Verified,
        PlanTaskStatus::Failed => NodeStatus::Failed,
        PlanTaskStatus::Killed => NodeStatus::Killed,
    }
}

fn chain_status(status: ChainStatus) -> NodeStatus {
    match status {
        ChainStatus::Pending | ChainStatus::Undone => NodeStatus::Pending,
        ChainStatus::Running => NodeStatus::Running,
        ChainStatus::Paused => NodeStatus::Paused,
        ChainStatus::Completed => NodeStatus::Verified,
        ChainStatus::Failed => NodeStatus::Failed,
        ChainStatus::Killed => NodeStatus::Killed,
    }
}

fn chain_step_status(status: ChainStepStatus) -> NodeStatus {
    match status {
        ChainStepStatus::Pending | ChainStepStatus::Undone => NodeStatus::Pending,
        ChainStepStatus::Running => NodeStatus::Running,
        ChainStepStatus::Completed => NodeStatus::Gated,
        ChainStepStatus::Failed => NodeStatus::Failed,
        ChainStepStatus::Skipped | ChainStepStatus::Applied => NodeStatus::Verified,
    }
}

fn campaign_status(status: CampaignStatus) -> NodeStatus {
    match status {
        CampaignStatus::Pending => NodeStatus::Pending,
        CampaignStatus::Forked => NodeStatus::Running,
        CampaignStatus::Merged => NodeStatus::Verified,
        CampaignStatus::Failed => NodeStatus::Failed,
        CampaignStatus::Killed => NodeStatus::Killed,
    }
}

fn sub_goal_status(status: SubGoalStatus) -> NodeStatus {
    match status {
        SubGoalStatus::Pending => NodeStatus::Pending,
        SubGoalStatus::Running => NodeStatus::Running,
        SubGoalStatus::Merged => NodeStatus::Verified,
        SubGoalStatus::Failed => NodeStatus::Failed,
        SubGoalStatus::Killed => NodeStatus::Killed,
    }
}

fn fold_plan_feed_event(model: &mut TreeModel, event: &PlanFeedEvent) {
    match event {
        PlanFeedEvent::Plan { event } => fold_plan_event(model, event),
        PlanFeedEvent::ChildRun {
            task_id,
            run_id,
            event,
        } => {
            fold_run_event(model, event, Some(task_id.as_str()));
            ensure_task_run_child(model, task_id, run_id, run_event_node_status(event));
        }
        PlanFeedEvent::RepairRun { run_id, event } => {
            fold_run_event(model, event, None);
            if model.find(&NodeId::run(run_id)).is_none() {
                model
                    .root
                    .children
                    .push(placeholder_run_node(run_id, run_event_node_status(event)));
            }
        }
        PlanFeedEvent::Snapshot { plan } => apply_plan_snapshot(model, plan),
        PlanFeedEvent::Warning { .. } => {}
    }
}

fn fold_plan_event(model: &mut TreeModel, event: &PlanEvent) {
    match &event.event {
        PlanEventKind::PlanCreated { .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Pending);
        }
        PlanEventKind::PlanStarted
        | PlanEventKind::MergeStarted
        | PlanEventKind::MergeRepairPlanned { .. }
        | PlanEventKind::MergeRepairStarted { .. }
        | PlanEventKind::MergeRepaired { .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Running);
        }
        PlanEventKind::TaskReady { task_id, .. } => {
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Pending,
            );
        }
        PlanEventKind::TaskStarted { task_id, .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Running);
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Running,
            );
        }
        // The node is going back around, so it reads as pending work rather
        // than a failure — the plan as a whole is still running.
        PlanEventKind::TaskRetrying { task_id, .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Running);
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Pending,
            );
        }
        PlanEventKind::CircuitBreakerTripped { .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Failed);
        }
        // Landing is the strongest completion signal a node has: gated,
        // applied, and on the branch.
        PlanEventKind::TaskApplied { task_id, .. } => {
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Verified,
            );
        }
        PlanEventKind::TaskRunDiscovered {
            task_id,
            run_id: Some(run_id),
            ..
        } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Running);
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Running,
            );
            ensure_task_run_child(model, task_id, run_id, NodeStatus::Running);
        }
        PlanEventKind::TaskRunDiscovered { task_id, .. } => {
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Running,
            );
        }
        PlanEventKind::TaskCompleted {
            task_id,
            run_id,
            status,
            ..
        } => {
            let status = status_from_text(status);
            set_status(model, &NodeId::task(&event.plan_id, task_id), status);
            if let Some(run_id) = run_id {
                ensure_task_run_child(model, task_id, run_id, status);
                set_status(model, &NodeId::run(run_id), status);
            }
        }
        PlanEventKind::TaskBlocked { task_id, .. } => {
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Paused,
            );
        }
        PlanEventKind::TaskFailed { task_id, .. } => {
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Failed,
            );
        }
        PlanEventKind::TaskBudgetExhausted { task_id, .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Failed);
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Failed,
            );
        }
        PlanEventKind::RootBudgetExhausted { .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Failed);
        }
        PlanEventKind::TaskKilled {
            task_id, run_id, ..
        } => {
            set_status(
                model,
                &NodeId::task(&event.plan_id, task_id),
                NodeStatus::Killed,
            );
            if let Some(run_id) = run_id {
                set_status(model, &NodeId::run(run_id), NodeStatus::Killed);
            }
        }
        PlanEventKind::MergeConflict { .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Paused);
        }
        PlanEventKind::MergeRepairRunDiscovered { run_id, .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Running);
            if model.find(&NodeId::run(run_id)).is_none() {
                model
                    .root
                    .children
                    .push(placeholder_run_node(run_id, NodeStatus::Running));
            }
        }
        PlanEventKind::MergeRepairFailed { .. } | PlanEventKind::PlanFailed { .. } => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Failed);
        }
        PlanEventKind::MergeCompleted { .. } | PlanEventKind::PlanCompleted => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Verified);
        }
        PlanEventKind::PlanKilled => {
            set_status(model, &NodeId::plan(&event.plan_id), NodeStatus::Killed);
        }
    }
}

fn apply_plan_snapshot(model: &mut TreeModel, plan: &Plan) {
    if let Some(root) = model.find_mut(&NodeId::plan(&plan.plan_id)) {
        root.status = plan_status(plan.status);
        for task in &plan.tasks {
            let task_id = NodeId::task(&plan.plan_id, &task.task_id);
            if let Some(node) = root.find_mut(&task_id) {
                node.label = task_label(task);
                node.status = plan_task_status(task.status);
            } else {
                root.children.push(task_node_for_snapshot(plan, task));
            }
        }
    } else {
        for task in &plan.tasks {
            let task_id = NodeId::task(&plan.plan_id, &task.task_id);
            if let Some(node) = model.find_mut(&task_id) {
                node.label = task_label(task);
                node.status = plan_task_status(task.status);
            }
        }
    }
}

fn task_node_for_snapshot(plan: &Plan, task: &PlanTask) -> TreeNode {
    let mut node = TreeNode::new(
        NodeId::task(&plan.plan_id, &task.task_id),
        NodeKind::Task,
        task_label(task),
        plan_task_status(task.status),
    );
    if let Some(run_id) = task.child_run_id.as_deref() {
        node.children
            .push(placeholder_run_node(run_id, plan_task_status(task.status)));
    }
    node
}

fn fold_run_event(model: &mut TreeModel, event: &RunEvent, parent_task_id: Option<&str>) {
    let status = run_event_node_status(event);
    set_status(model, &NodeId::run(&event.run_id), status);
    if let Some(node) = model.find_mut(&NodeId::run(&event.run_id))
        && let RunEventKind::SpendDelta { total_cost_usd, .. } = event.event
    {
        node.spend = Some(total_cost_usd);
    }
    if let Some(task_id) = parent_task_id
        && matches!(
            event.event,
            RunEventKind::RunCompleted { .. } | RunEventKind::Error { .. }
        )
        && let Some(task) = model.root.find_task_mut(task_id)
    {
        task.status = status;
    }
}

fn run_event_node_status(event: &RunEvent) -> NodeStatus {
    match &event.event {
        RunEventKind::RunCompleted { status } => status_from_text(status),
        RunEventKind::Error { .. } => NodeStatus::Failed,
        RunEventKind::TurnStarted { .. }
        | RunEventKind::ToolCallStarted { .. }
        | RunEventKind::ToolCallResult { .. }
        | RunEventKind::TokenUsageDelta { .. }
        | RunEventKind::SpendDelta { .. }
        | RunEventKind::DocsCheckpoint { .. }
        | RunEventKind::RunPromoted { .. } => NodeStatus::Running,
    }
}

fn fold_chain_event(model: &mut TreeModel, event: &ChainEvent) {
    match event.event {
        ChainEventKind::ChainCreated => {
            set_status(model, &NodeId::chain(&event.chain_id), NodeStatus::Pending);
        }
        ChainEventKind::ChainStepStarted
        | ChainEventKind::ChainApplyStarted
        | ChainEventKind::ChainResumed
        | ChainEventKind::ChainRunCompleted
        | ChainEventKind::ChainHookInvoked
        | ChainEventKind::ChainStepExtended
        | ChainEventKind::ChainStepRedone
        | ChainEventKind::LegacyExecutionSelected => {
            set_status(model, &NodeId::chain(&event.chain_id), NodeStatus::Running);
            if let Some(step_index) = event.step_index {
                set_status(
                    model,
                    &NodeId::chain_step(&event.chain_id, step_index),
                    chain_event_step_status(event),
                );
            }
        }
        ChainEventKind::ChainApplied => {
            if let Some(step_index) = event.step_index {
                set_status(
                    model,
                    &NodeId::chain_step(&event.chain_id, step_index),
                    NodeStatus::Verified,
                );
            }
        }
        ChainEventKind::ChainApplyRefused | ChainEventKind::ChainPaused => {
            set_status(model, &NodeId::chain(&event.chain_id), NodeStatus::Paused);
        }
        ChainEventKind::ChainStepFailed => {
            if let Some(step_index) = event.step_index {
                set_status(
                    model,
                    &NodeId::chain_step(&event.chain_id, step_index),
                    NodeStatus::Failed,
                );
            }
        }
        ChainEventKind::ChainKilled => {
            set_status(model, &NodeId::chain(&event.chain_id), NodeStatus::Killed);
        }
        ChainEventKind::ChainCompleted => {
            set_status(model, &NodeId::chain(&event.chain_id), NodeStatus::Verified);
        }
        ChainEventKind::ChainUndoStarted | ChainEventKind::ChainUndoneStep => {
            if let Some(step_index) = event.step_index {
                set_status(
                    model,
                    &NodeId::chain_step(&event.chain_id, step_index),
                    NodeStatus::Pending,
                );
            }
        }
    }
}

fn chain_event_step_status(event: &ChainEvent) -> NodeStatus {
    match event.event {
        ChainEventKind::ChainRunCompleted => NodeStatus::Gated,
        _ => NodeStatus::Running,
    }
}

fn fold_campaign_feed_event(model: &mut TreeModel, event: &CampaignFeedEvent) {
    match event {
        CampaignFeedEvent::Campaign { event } => fold_campaign_event(model, event),
        CampaignFeedEvent::SubPlan { sub_id, event } => {
            fold_plan_event(model, event);
            if matches!(
                event.event,
                PlanEventKind::PlanStarted
                    | PlanEventKind::TaskStarted { .. }
                    | PlanEventKind::TaskRunDiscovered { .. }
            ) {
                set_sub_status_by_id(model, sub_id, NodeStatus::Running);
            }
        }
        CampaignFeedEvent::Snapshot { campaign } => apply_campaign_snapshot(model, campaign),
        CampaignFeedEvent::Warning { .. } => {}
    }
}

fn fold_campaign_event(model: &mut TreeModel, event: &CampaignEvent) {
    match event.kind.as_str() {
        "campaign_created" => {
            if let Some(root) = campaign_root_mut(model) {
                root.status = NodeStatus::Pending;
            }
        }
        "campaign_started"
        | "campaign_merge_conflict"
        | "campaign_repair_planned"
        | "campaign_repair_started"
        | "campaign_repaired" => {
            if let Some(root) = campaign_root_mut(model) {
                root.status = NodeStatus::Running;
            }
        }
        "campaign_completed" => {
            if let Some(root) = campaign_root_mut(model) {
                root.status = NodeStatus::Verified;
            }
        }
        "rollup_refused" | "campaign_repair_failed" => {
            if let Some(root) = campaign_root_mut(model) {
                root.status = NodeStatus::Failed;
            }
        }
        "budget_exhausted" => {
            if let Some(root) = campaign_root_mut(model) {
                root.status = NodeStatus::Paused;
            }
        }
        "sub_launched" => {
            if let Some(sub_id) = event
                .detail
                .get("sub_id")
                .and_then(serde_json::Value::as_str)
            {
                set_sub_status_by_id(model, sub_id, NodeStatus::Running);
            }
        }
        "sub_merged" => {
            if let Some(sub_id) = event
                .detail
                .get("sub_id")
                .and_then(serde_json::Value::as_str)
            {
                set_sub_status_by_id(model, sub_id, NodeStatus::Verified);
            }
        }
        "sub_failed" => {
            if let Some(sub_id) = event
                .detail
                .get("sub_id")
                .and_then(serde_json::Value::as_str)
            {
                set_sub_status_by_id(model, sub_id, NodeStatus::Failed);
            }
        }
        _ => {}
    }
}

fn apply_campaign_snapshot(model: &mut TreeModel, campaign: &Campaign) {
    let Some(root) = campaign_root_mut(model) else {
        return;
    };
    root.status = campaign_status(campaign.status);
    for sub in &campaign.sub_goals {
        let id = NodeId::sub_goal(&campaign.campaign_id, &sub.sub_id);
        if let Some(node) = root.find_mut(&id) {
            node.label = sub.goal.clone();
            node.status = sub_goal_status(sub.status);
        } else {
            root.children.push(TreeNode::new(
                id,
                NodeKind::SubGoal,
                sub.goal.clone(),
                sub_goal_status(sub.status),
            ));
        }
    }
}

fn campaign_root_mut(model: &mut TreeModel) -> Option<&mut TreeNode> {
    if model.root.kind == NodeKind::Campaign {
        Some(&mut model.root)
    } else {
        None
    }
}

fn set_sub_status_by_id(model: &mut TreeModel, sub_id: &str, status: NodeStatus) {
    fn visit(node: &mut TreeNode, sub_id: &str, status: NodeStatus) -> bool {
        if let NodeId::SubGoal { sub_id: id, .. } = &node.id
            && id == sub_id
        {
            node.status = status;
            return true;
        }
        node.children
            .iter_mut()
            .any(|child| visit(child, sub_id, status))
    }
    visit(&mut model.root, sub_id, status);
}

fn ensure_task_run_child(model: &mut TreeModel, task_id: &str, run_id: &str, status: NodeStatus) {
    let Some(task) = model.root.find_task_mut(task_id) else {
        return;
    };
    if let Some(node) = task
        .children
        .iter_mut()
        .find(|child| child.id == NodeId::run(run_id))
    {
        node.status = status;
    } else {
        task.children.push(placeholder_run_node(run_id, status));
    }
}

fn set_status(model: &mut TreeModel, id: &NodeId, status: NodeStatus) {
    if let Some(node) = model.find_mut(id) {
        node.status = status;
    }
}

fn status_from_text(status: &str) -> NodeStatus {
    let status = status.to_ascii_lowercase();
    if status.contains("killed") {
        NodeStatus::Killed
    } else if status.contains("failed") || status.contains("error") || status.contains("refused") {
        NodeStatus::Failed
    } else if status.contains("paused") || status.contains("blocked") {
        NodeStatus::Paused
    } else if status.contains("completed")
        || status.contains("verified")
        || status.contains("accepted")
        || status.contains("merged")
    {
        NodeStatus::Verified
    } else {
        NodeStatus::Running
    }
}
