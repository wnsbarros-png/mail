//! Plain-text templates. Values are data, never recursively expanded or evaluated.
use crate::{
    Core,
    db::repo,
    error::{CoreError, Result},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub type Variables = BTreeMap<String, String>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rendered {
    pub subject: String,
    pub body: String,
    pub missing: Vec<String>,
}
fn error(s: &str) -> CoreError {
    CoreError::Other(s.into())
}
fn valid_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}
fn append(out: &mut String, text: &str) -> Result<()> {
    if out.len() + text.len() > 128 * 1024 {
        return Err(error("Rendered template exceeds 128 KiB"));
    }
    out.push_str(text);
    Ok(())
}
fn expand(text: &str, vars: &Variables, missing: &mut BTreeSet<String>) -> Result<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("{{") {
        append(&mut out, &rest[..at])?;
        let tail = &rest[at + 2..];
        let Some(end) = tail.find("}}") else {
            append(&mut out, &rest[at..])?;
            return Ok(out);
        };
        let key = tail[..end].trim();
        if valid_key(key) {
            match vars.get(key).filter(|v| !v.trim().is_empty()) {
                Some(value) => append(&mut out, value)?,
                None => {
                    missing.insert(key.into());
                    append(&mut out, &rest[at..at + 2 + end + 2])?;
                }
            }
        } else {
            append(&mut out, &rest[at..at + 2 + end + 2])?;
        }
        rest = &tail[end + 2..];
    }
    append(&mut out, rest)?;
    Ok(out)
}
pub fn render(subject: &str, body: &str, vars: &Variables) -> Result<Rendered> {
    if vars.len() > 100
        || vars
            .iter()
            .any(|(k, v)| !valid_key(k) || v.len() > 16 * 1024)
        || subject.len() > 16 * 1024
        || body.len() > 256 * 1024
    {
        return Err(error("Template/variables exceed limits"));
    }
    let mut missing = BTreeSet::new();
    let subject = expand(subject, vars, &mut missing)?;
    let body = expand(body, vars, &mut missing)?;
    // A value cannot smuggle another unfilled variable past the send gate.
    missing.extend(unfilled(&subject, &body));
    if subject.contains(['\r', '\n']) {
        return Err(error("Template subject cannot contain line breaks"));
    }
    if subject.len() > 16 * 1024 || body.len() > 128 * 1024 {
        return Err(error("Rendered template too large"));
    }
    Ok(Rendered {
        subject,
        body,
        missing: missing.into_iter().collect(),
    })
}
pub fn unfilled(subject: &str, body: &str) -> BTreeSet<String> {
    let mut missing = BTreeSet::new();
    for text in [subject, body] {
        let mut rest = text;
        while let Some(at) = rest.find("{{") {
            let tail = &rest[at + 2..];
            let Some(end) = tail.find("}}") else {
                break;
            };
            let key = tail[..end].trim();
            if valid_key(key) {
                missing.insert(key.to_string());
            }
            rest = &tail[end + 2..];
        }
    }
    missing
}
pub fn ensure_filled(subject: &str, body: &str) -> Result<()> {
    let missing = unfilled(subject, body);
    if !missing.is_empty() {
        return Err(error(&format!(
            "Preencha as variáveis / Fill template variables: {}",
            missing.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(())
}
/// Human-friendly editor: one `key=value` per line, values may contain '='.
pub fn parse_values(text: &str) -> Result<Variables> {
    let mut vars = Variables::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (k, v) = line.split_once('=').ok_or_else(|| {
            error("Use nome=valor, uma variável por linha / one variable per line")
        })?;
        let k = k.trim();
        if !valid_key(k) || vars.insert(k.into(), v.trim().into()).is_some() {
            return Err(error("Invalid or duplicate template variable"));
        }
    }
    if vars.len() > 100 {
        return Err(error("Too many template variables"));
    }
    Ok(vars)
}
pub const FOLLOWUP_BODY: &str = "Olá, {{nome}},\n\nGostaria de confirmar se recebeu minha mensagem anterior. Quando puder, poderia me dar um retorno? Fico à disposição.\n\n{{assinatura}}";
impl Core {
    /// Seed once, not whenever an example is edited/deleted. No invented business identity.
    pub async fn seed_template_examples(&self) -> Result<()> {
        self.db.write(|c|{c.execute_batch("CREATE TABLE IF NOT EXISTS ai_template_seeds(version INTEGER PRIMARY KEY);")?;let tx=c.transaction()?;let done=tx.execute("INSERT OR IGNORE INTO ai_template_seeds(version) VALUES(1)",[])?>0;if done{for(name,subject,body)in[
 ("Exemplo editável · Orçamento","Orçamento · {{empresa}}","Olá, {{nome}},\n\nSegue o orçamento no valor de {{valor}}, válido até {{data}}. Fico à disposição.\n\n{{assinatura}}"),
 ("Exemplo editável · Segunda via de boleto","Segunda via de boleto","Olá, {{nome}},\n\nA segunda via solicitada tem valor de {{valor}} e vencimento em {{vencimento}}. Confira os dados e anexe o documento correto antes de enviar.\n\n{{assinatura}}"),
 ("Exemplo editável · Confirmação de pagamento","Confirmação de pagamento","Olá, {{nome}},\n\nConfirmamos, após conferência, o pagamento de {{valor}} em {{data}}. Obrigado.\n\n{{assinatura}}"),
 ("Exemplo editável · Agradecimento","Obrigado pelo contato","Olá, {{nome}},\n\nAgradecemos pelo contato com {{empresa}}. Estamos à disposição.\n\n{{assinatura}}"),
 ("Exemplo editável · Follow-up","Retorno da mensagem anterior",FOLLOWUP_BODY)]{repo::snippets::save(&tx,None,name,None,Some(subject),body)?;}}tx.commit()?;Ok(())}).await
    }
    pub async fn template_signature(&self, account: i64, reply: bool) -> Result<Option<String>> {
        self.db
            .read(move |c| {
                let settings = repo::settings::get(c)?;
                let id = settings
                    .signature_defaults
                    .get(&account.to_string())
                    .and_then(|d| {
                        if reply {
                            d.reply_id.as_ref()
                        } else {
                            d.new_id.as_ref()
                        }
                    });
                Ok(id
                    .and_then(|id| {
                        settings
                            .signature_list
                            .iter()
                            .find(|s| s.account_id == account && &s.id == id)
                    })
                    .map(crate::signatures::plain_text)
                    .filter(|s| !s.trim().is_empty()))
            })
            .await
    }
    pub async fn render_email_template(
        &self,
        id: i64,
        account: Option<i64>,
        reply: bool,
        mut vars: Variables,
    ) -> Result<Rendered> {
        let template = self
            .list_snippets()
            .await?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| error("Template not found"))?;
        if !vars.contains_key("assinatura")
            && let Some(account) = account
            && let Some(signature) = self.template_signature(account, reply).await?
        {
            vars.insert("assinatura".into(), signature);
        }
        render(
            template.subject.as_deref().unwrap_or_default(),
            &template.body_text,
            &vars,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variables_matrix() {
        let vars = Variables::from([
            ("nome".into(), "Ana & <Cliente>".into()),
            ("custom".into(), "A=B".into()),
        ]);
        let r = render(
            "Olá {{ nome }}",
            "{{nome}} {{valor}} {{custom}} {{valor}}",
            &vars,
        )
        .unwrap();
        assert_eq!(r.subject, "Olá Ana & <Cliente>");
        assert_eq!(r.missing, vec!["valor"]);
        assert!(r.body.contains("A=B"));
        assert!(ensure_filled(&r.subject, &r.body).is_err());
        assert!(render("a\nb", "", &vars).is_err());
    }
    #[test]
    fn values_are_not_recursive_or_executable() {
        let r = render(
            "",
            "{{nome}}",
            &Variables::from([("nome".into(), "{{empresa}}".into())]),
        )
        .unwrap();
        assert_eq!(r.body, "{{empresa}}");
        assert_eq!(r.missing, vec!["empresa"]);
        assert!(parse_values("nome=a\nnome=b").is_err());
        assert_eq!(parse_values("custom=A=B").unwrap()["custom"], "A=B");
    }
}
