//! Local-only follow-up tracking. This module never queues or sends mail.
use crate::{
    Core,
    error::{CoreError, Result},
    templates::{self, Rendered, Variables},
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Followup {
    pub message_id: i64,
    pub account_id: i64,
    pub subject: String,
    pub due_ms: i64,
    pub state: String,
    pub notified: bool,
    pub template_id: Option<i64>,
}
fn error(s: &str) -> CoreError {
    CoreError::Other(s.into())
}
fn schema(c: &rusqlite::Connection) -> Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS ai_followups(message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,due_ms INTEGER NOT NULL,state TEXT NOT NULL DEFAULT 'waiting',notified INTEGER NOT NULL DEFAULT 0,template_id INTEGER REFERENCES snippets(id) ON DELETE SET NULL);")?;
    Ok(())
}
fn refresh(c: &rusqlite::Connection) -> Result<()> {
    c.execute("UPDATE ai_followups SET state='answered' WHERE state IN ('waiting','notified','drafted') AND EXISTS(SELECT 1 FROM messages original JOIN messages reply ON reply.thread_id=original.thread_id AND reply.account_id=original.account_id WHERE original.id=ai_followups.message_id AND reply.is_outgoing=0 AND reply.is_draft=0 AND (CASE WHEN reply.date BETWEEN -99999999999 AND 99999999999 THEN reply.date*1000 ELSE reply.date END)>(CASE WHEN original.date BETWEEN -99999999999 AND 99999999999 THEN original.date*1000 ELSE original.date END))",[])?;
    Ok(())
}
impl Core {
    pub async fn set_followup(&self, message: i64, days: i64, template: Option<i64>) -> Result<()> {
        if !(1..=365).contains(&days) {
            return Err(error("Follow-up days must be 1–365"));
        }
        let due = chrono::Utc::now().timestamp_millis() + days * 86400000;
        self.db.write(move|c|{schema(c)?;let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM messages m LEFT JOIN folders f ON f.id=m.folder_id WHERE m.id=?1 AND m.is_outgoing=1 AND m.is_draft=0 AND m.thread_id IS NOT NULL AND (f.role='sent' OR EXISTS(SELECT 1 FROM pending_actions a WHERE a.message_id=m.id AND a.kind='send' AND a.state='done')))",[message],|r|r.get(0))?;if !valid{return Err(error("Select an actually sent message with a thread / Selecione uma mensagem enviada com conversa"));}
if let Some(id)=template{let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM snippets WHERE id=?1)",[id],|r|r.get(0))?;if !exists{return Err(error("Template not found"));}}c.execute("INSERT INTO ai_followups(message_id,due_ms,template_id) VALUES(?1,?2,?3) ON CONFLICT(message_id) DO UPDATE SET due_ms=excluded.due_ms,template_id=excluded.template_id,state='waiting',notified=0",rusqlite::params![message,due,template])?;refresh(c)?;Ok(())}).await
    }
    pub async fn cancel_followup(&self, message: i64) -> Result<()> {
        self.db
            .write(move |c| {
                schema(c)?;
                c.execute(
                    "UPDATE ai_followups SET state='cancelled' WHERE message_id=?1",
                    [message],
                )?;
                Ok(())
            })
            .await
    }
    pub async fn list_followups(&self, account: Option<i64>) -> Result<Vec<Followup>> {
        self.db.write(move|c|{schema(c)?;refresh(c)?;let mut q=c.prepare("SELECT m.id,m.account_id,substr(m.subject,1,300),f.due_ms,f.state,f.notified,f.template_id FROM ai_followups f JOIN messages m ON m.id=f.message_id WHERE (?1 IS NULL OR m.account_id=?1) ORDER BY f.due_ms,m.id LIMIT 200")?;Ok(q.query_map([account],|r|Ok(Followup{message_id:r.get(0)?,account_id:r.get(1)?,subject:r.get(2)?,due_ms:r.get(3)?,state:r.get(4)?,notified:r.get(5)?,template_id:r.get(6)?}))?.collect::<std::result::Result<Vec<_>,_>>()?)}).await
    }
    /// Claim notices before delivery: at-most-once and never a send or autonomous draft.
    pub async fn claim_followup_notices(&self) -> Result<usize> {
        let now = chrono::Utc::now().timestamp_millis();
        self.db.write(move|c|{schema(c)?;refresh(c)?;Ok(c.execute("UPDATE ai_followups SET state='notified',notified=1 WHERE state='waiting' AND notified=0 AND due_ms<=?1",[now])?)}).await
    }
    pub async fn followup_suggestion(&self, message: i64) -> Result<Rendered> {
        let list = self.list_followups(None).await?;
        let f = list
            .into_iter()
            .find(|f| {
                f.message_id == message
                    && f.due_ms <= chrono::Utc::now().timestamp_millis()
                    && matches!(f.state.as_str(), "waiting" | "notified")
            })
            .ok_or_else(|| error("Follow-up inactive or already answered"))?;
        let (name,) = self
            .db
            .read(move |c| {
                let to: String =
                    c.query_row("SELECT to_json FROM messages WHERE id=?1", [message], |r| {
                        r.get(0)
                    })?;
                let addresses: Vec<crate::models::Address> = serde_json::from_str(&to)?;
                Ok((addresses
                    .first()
                    .and_then(|a| a.name.clone())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "pessoal".into()),))
            })
            .await?;
        let signature = self
            .template_signature(f.account_id, true)
            .await?
            .unwrap_or_else(|| "Atenciosamente".into());
        let vars = Variables::from([("nome".into(), name), ("assinatura".into(), signature)]);
        if let Some(id) = f.template_id {
            self.render_email_template(id, Some(f.account_id), true, vars)
                .await
        } else {
            templates::render("", templates::FOLLOWUP_BODY, &vars)
        }
    }
    pub async fn mark_followup_drafted(&self, message: i64) -> Result<bool> {
        self.db.write(move|c|{schema(c)?;refresh(c)?;Ok(c.execute("UPDATE ai_followups SET state='drafted' WHERE message_id=?1 AND state IN ('waiting','notified')",[message])?>0)}).await
    }
    pub async fn reset_followup_draft_failure(&self, message: i64) -> Result<()> {
        self.db.write(move|c|{c.execute("UPDATE ai_followups SET state='notified' WHERE message_id=?1 AND state='drafted'",[message])?;Ok(())}).await
    }
    pub async fn followup_account(&self, message: i64) -> Result<Option<i64>> {
        self.db
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT account_id FROM messages WHERE id=?1",
                    [message],
                    |r| r.get(0),
                )
                .optional()?)
            })
            .await
    }
}
