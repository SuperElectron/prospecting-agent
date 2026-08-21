use crate::llm::client::{ChatMessage, LlmClient, LlmError};
use crate::llm::governance::{Policy, inject};

#[derive(Debug, Clone, PartialEq)]
pub struct StepPlan {
    pub system: String,
    pub context: String,
    pub steps: Vec<String>,
}

impl StepPlan {
    pub fn new(system: impl Into<String>, context: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            context: context.into(),
            steps: Vec::new(),
        }
    }

    #[must_use]
    pub fn step(mut self, instruction: impl Into<String>) -> Self {
        self.steps.push(instruction.into());
        self
    }

    pub fn messages_for_step(
        &self,
        index: usize,
        policies: &[Policy],
        prior_answers: &[String],
    ) -> Vec<ChatMessage> {
        let mut messages = vec![ChatMessage::system(&self.system)];
        if !self.context.is_empty() {
            messages.push(ChatMessage::user(format!("Context:\n{}", self.context)));
        }
        for (i, answer) in prior_answers.iter().enumerate() {
            if let Some(instruction) = self.steps.get(i) {
                messages.push(ChatMessage::user(instruction.clone()));
            }
            messages.push(ChatMessage::assistant(answer.clone()));
        }
        if let Some(instruction) = self.steps.get(index) {
            messages.push(ChatMessage::user(inject(policies, instruction)));
        }
        messages
    }

    pub async fn run(&self, client: &LlmClient, policies: &[Policy]) -> Result<Vec<String>, LlmError> {
        let mut answers = Vec::with_capacity(self.steps.len());
        for index in 0..self.steps.len() {
            let messages = self.messages_for_step(index, policies, &answers);
            let answer = client.chat(&messages).await?;
            answers.push(answer);
        }
        Ok(answers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::governance::defaults;

    #[test]
    fn first_step_carries_system_context_and_instruction() {
        let plan = StepPlan::new("You are a helper.", "Company: Acme")
            .step("Analyze the contact")
            .step("Write the email");
        let messages = plan.messages_for_step(0, &[], &[]);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "system");
        assert!(messages[1].content.contains("Acme"));
        assert_eq!(messages[2].content, "Analyze the contact");
    }

    #[test]
    fn later_steps_thread_prior_answers_as_assistant_turns() {
        let plan = StepPlan::new("sys", "ctx").step("one").step("two");
        let prior = vec!["answer one".to_string()];
        let messages = plan.messages_for_step(1, &[], &prior);
        let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["system", "user", "user", "assistant", "user"]);
        assert_eq!(messages[3].content, "answer one");
        assert_eq!(messages[4].content, "two");
    }

    #[test]
    fn governance_applies_to_the_active_instruction() {
        let plan = StepPlan::new("sys", "").step("Write the outreach email");
        let messages = plan.messages_for_step(0, &defaults(), &[]);
        let last = &messages.last().unwrap().content;
        assert!(last.contains("Outbound Voice"));
        assert!(last.ends_with("Write the outreach email"));
    }
}
