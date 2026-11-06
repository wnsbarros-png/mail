# Integração com IA (MCP) / AI integration (MCP)

[Português](#português) · [English](#english)

---

## Português

Esta versão inclui o `mail-mcp`, um servidor **MCP** (Model Context Protocol) local
que permite a um assistente de IA (Claude Desktop, Cursor, Grok Bot, VS Code e
outros clientes MCP) **ler, procurar, enviar, responder, excluir e monitorar**
e-mails das contas configuradas no aplicativo.

### Como funciona

- O `mail-mcp` usa o **mesmo banco de dados local e as mesmas credenciais** (guardadas no
  Gerenciador de Credenciais do Windows / Keychain / Secret Service) do aplicativo.
  Por isso funciona com tudo que o app já suporta: Gmail e Outlook (login OAuth),
  e qualquer servidor IMAP/SMTP (GoDaddy, Hostinger, Zoho, etc.), JMAP e POP3.
- Comunicação só por **stdio** (entrada/saída do processo): nenhuma porta de rede é aberta.
  Só o programa que iniciou o `mail-mcp` consegue falar com ele.
- Credenciais nunca são enviadas ao assistente nem gravadas em log. Logs vão para stderr (`RUST_LOG`).

### Ferramentas

| Ferramenta | O que faz |
| --- | --- |
| `list_accounts` | Lista contas e estado de sincronização |
| `list_folders` | Lista pastas de uma conta |
| `list_messages` | Lista mensagens (inbox, sent, drafts, archive, trash, spam, starred, all ou `folderId`), com filtro `unreadOnly` |
| `search_messages` | Busca de texto com a sintaxe do app (`from:`, `subject:`, `is:unread`, `has:attachment`...) |
| `read_message` | Lê cabeçalhos, corpo em texto e anexos (baixa o corpo do servidor se preciso) |
| `send_message` | Novo e-mail ⚠️ confirmação |
| `reply_message` | Responder / responder a todos, citando o original ⚠️ confirmação |
| `delete_message` | Move para a Lixeira (exclusão permanente só se habilitada) ⚠️ confirmação |
| `mark_read` | Marca como lida / não lida |
| `get_new_messages` | Monitoramento: devolve o que chegou depois de um `cursor` (pode esperar até 120 s) |
| `sync_now` | Pede verificação imediata no servidor |
| `list_pending_actions`, `confirm_action`, `cancel_action` | Fluxo de confirmação |

### Confirmação (padrão: ligada)

Enviar, responder e excluir acontecem em **duas etapas**:

1. A ferramenta (ex.: `send_message`) cria um **rascunho real** (aparece em Rascunhos no app)
   e devolve `status: "pending_confirmation"`, um `pendingId` e o resumo (destinatários, assunto, texto).
2. Nada sai do computador até o assistente chamar `confirm_action` com esse `pendingId`.
   `cancel_action` descarta (e apaga o rascunho). Pendências expiram em 30 minutos.

Recomendação: no seu cliente MCP, deixe `confirm_action` como **"sempre perguntar"**, assim
é você quem aprova cada envio/exclusão. As configurações ficam em `ai-mcp.json`, na pasta de
dados do app (no Windows: `%APPDATA%\flectar-mail\ai-mcp.json`, criado na primeira execução):

```json
{
  "ai_actions_require_confirmation": true,
  "allow_permanent_delete": false
}
```

Mude `ai_actions_require_confirmation` para `false` para o assistente agir direto
(não recomendado). Um arquivo inválido volta sempre para o padrão seguro.

### Como testar no Windows

1. Instale o Rust (https://rustup.rs) com as *Visual Studio C++ Build Tools*.
2. Clone o fork e compile só o servidor (bem mais rápido que o app inteiro):
   ```powershell
   git clone -b feat/ai-mcp https://github.com/wnsbarros-png/mail
   cd mail
   cargo build --release -p mail-mcp
   ```
   O executável fica em `target\release\mail-mcp.exe`.
3. Adicione suas contas pelo aplicativo (compilado com `cargo build --release`, ou o
   instalador oficial do Flectar Mail 0.1.0-alpha.7, que usa a mesma pasta de dados).
4. Feche o aplicativo e configure o cliente MCP (exemplo para Claude Desktop / Cursor):
   ```json
   {
     "mcpServers": {
       "email": {
         "command": "C:\\caminho\\para\\mail\\target\\release\\mail-mcp.exe",
         "args": []
       }
     }
   }
   ```
   Opções: `--data-dir PASTA` e `--cache-dir PASTA` para usar outro perfil.

### Adicionar contas GoDaddy e Hostinger (IMAP/SMTP)

No app: **Adicionar conta → outro provedor / IMAP**, usuário = e-mail completo, senha do e-mail.

| Provedor | Entrada (IMAP) | Saída (SMTP) |
| --- | --- | --- |
| GoDaddy (Professional Email / Titan) | `imap.secureserver.net`, porta 993, SSL/TLS | `smtpout.secureserver.net`, porta 465, SSL/TLS (ou 587) |
| Hostinger Email | `imap.hostinger.com`, porta 993, SSL/TLS | `smtp.hostinger.com`, porta 465, SSL/TLS (ou 587 STARTTLS) |

Fontes: [GoDaddy](https://www.godaddy.com/help/use-imap-settings-to-add-my-professional-email-to-a-client-32204),
[Hostinger](https://docs.hostinger.com/emails/setup-devices). Se a conta Hostinger tiver
2FA, use uma senha de app. Contas GoDaddy hospedadas no Microsoft 365 devem ser adicionadas como **Outlook/Microsoft**.

Gmail e Outlook: use os botões de login do app (OAuth).

### Limitações conhecidas

- **Não rode o app e o `mail-mcp` ao mesmo tempo no mesmo perfil.** Cada um inicia seu próprio
  motor de sincronização; o SQLite aguenta o acesso simultâneo, mas dois motores podem
  duplicar trabalho. Um lock impede dois `mail-mcp` no mesmo perfil, mas ainda não há
  detecção do app aberto. (Próximo passo: o app expor o MCP quando estiver aberto.)
- A confirmação acontece pelo próprio assistente/cliente MCP; ainda não há notificação
  de aprovação dentro da interface do app (os e-mails pendentes aparecem em Rascunhos).
- Adicionar contas e o login OAuth são feitos no app, não pelo MCP.
- Sem envio de anexos ainda; leitura lista os anexos (metadados).
- Busca e listagem usam o que já foi sincronizado localmente.

---

## English

This build ships `mail-mcp`, a local **MCP** (Model Context Protocol) server that lets
an AI assistant **read, search, send, reply to, delete and monitor** email in the
accounts configured in the app.

- It reuses the app's **local database and OS-keyring credentials**, so every account
  type the app supports works: Gmail and Outlook (OAuth), any IMAP/SMTP server
  (GoDaddy, Hostinger, ...), JMAP and POP3.
- Transport is **stdio only**: no network port is opened. Credentials are never returned
  to the assistant or logged; logs go to stderr (`RUST_LOG`).

**Tools:** `list_accounts`, `list_folders`, `list_messages`, `search_messages`,
`read_message`, `send_message`*, `reply_message`*, `delete_message`*, `mark_read`,
`get_new_messages` (cursor-based monitoring, optional long-poll up to 120 s),
`sync_now`, `list_pending_actions`, `confirm_action`, `cancel_action`.

**Confirmation (on by default).** Tools marked * only stage a *pending action*: sends
create a real draft (visible in Drafts) and return a `pendingId` plus a summary. Nothing
is sent or deleted until `confirm_action(pendingId)`; `cancel_action` discards it (and
the draft). Pending actions expire after 30 minutes. Configure your MCP client to always
ask before `confirm_action`. Settings live in `ai-mcp.json` in the app data directory
(`%APPDATA%\flectar-mail` on Windows, `~/.local/share/flectar-mail` on Linux,
`~/Library/Application Support/flectar-mail` on macOS):
`ai_actions_require_confirmation` (default `true`) and `allow_permanent_delete`
(default `false`). An invalid file falls back to the safe defaults.

**Build and connect:** `cargo build --release -p mail-mcp`, add accounts in the app,
close the app, then point your MCP client at the binary:

```json
{ "mcpServers": { "email": { "command": "/path/to/target/release/mail-mcp", "args": [] } } }
```

**GoDaddy:** IMAP `imap.secureserver.net:993` SSL, SMTP `smtpout.secureserver.net:465` SSL (or 587).
**Hostinger:** IMAP `imap.hostinger.com:993` SSL, SMTP `smtp.hostinger.com:465` SSL (or 587 STARTTLS).
Username is the full address.

**Known limitations:** don't run the app and `mail-mcp` on the same profile at the same
time (two sync engines; only a second `mail-mcp` is locked out); confirmation happens in
the MCP client, not yet in the app UI; no outgoing attachments; account setup and OAuth
sign-in happen in the app.
