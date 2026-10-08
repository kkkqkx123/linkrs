#[derive(Debug, Clone)]
struct ConditionalState {
    condition_met: bool,
    any_branch_taken: bool,
    in_active_branch: bool,
}

#[derive(Debug, Clone)]
pub struct ConditionalStack {
    stack: Vec<ConditionalState>,
}

impl Default for ConditionalStack {
    fn default() -> Self {
        Self::new()
    }
}

impl ConditionalStack {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    pub fn push_if(&mut self, condition_met: bool) {
        let in_active = self.is_active() && condition_met;
        self.stack.push(ConditionalState {
            condition_met,
            any_branch_taken: condition_met,
            in_active_branch: in_active,
        });
    }

    pub fn push_elif(&mut self, condition_met: bool) {
        let parent_active = self.is_parent_active();
        if let Some(state) = self.stack.last_mut() {
            if state.any_branch_taken {
                state.in_active_branch = false;
            } else if condition_met {
                state.condition_met = true;
                state.any_branch_taken = true;
                state.in_active_branch = parent_active;
            } else {
                state.in_active_branch = false;
            }
        }
    }

    pub fn push_else(&mut self) {
        let parent_active = self.is_parent_active();
        if let Some(state) = self.stack.last_mut() {
            if state.any_branch_taken {
                state.in_active_branch = false;
            } else {
                state.condition_met = true;
                state.any_branch_taken = true;
                state.in_active_branch = parent_active;
            }
        }
    }

    pub fn pop(&mut self) {
        self.stack.pop();
    }

    pub fn is_active(&self) -> bool {
        if self.stack.is_empty() {
            return true;
        }
        self.stack.iter().all(|s| s.in_active_branch)
    }

    fn is_parent_active(&self) -> bool {
        if self.stack.len() <= 1 {
            return true;
        }
        self.stack[..self.stack.len() - 1]
            .iter()
            .all(|s| s.in_active_branch)
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}
