# Integração com IA (MCP) / AI integration (MCP)

## Português (Brasil)

O `mail-mcp` permite ler, procurar, enviar, responder, excluir e acompanhar e-mails
pelo protocolo MCP. As contas são adicionadas no aplicativo. O suporte aos
provedores vem do motor existente: Gmail/Outlook via OAuth e provedores IMAP/SMTP
(como GoDaddy/Hostinger), JMAP e POP3. Esta integração ainda não foi validada em
contas reais nem no Windows; não é uma promessa de compatibilidade já testada.

### Aplicativo aberto: um único motor e aprovação na interface

1. Inicie **esta versão modificada do aplicativo primeiro** e aguarde a caixa carregar.
2. Inicie o cliente MCP apontando para `mail-mcp`.
3. A ponte stdio conecta ao aplicativo e reutiliza seu motor de sincronização,
   banco e credenciais do sistema. Não inicia um segundo motor.
4. Enviar, responder e excluir criam propostas. Em até aproximadamente 750 ms,
   o painel **Ações da IA aguardando revisão** aparece na janela. Ele mostra uma
   proposta por vez, o total pendente e o resumo completo em área rolável:
   conta, destinatários, assunto, corpo e nomes/tipos de anexos para envios;
   IDs das mensagens para exclusões. Revise as mensagens pelos IDs antes de excluir.
5. Clique **Aprovar** ou **Cancelar** (botões somente texto). Aprovar enfileira o
   envio/exclusão; a sincronização executa a operação. Cancelar descarta a
   proposta e remove o rascunho de envio. O painel avança para a próxima proposta.

Por padrão, `confirm_action` é **recusado via MCP quando o app está aberto**.
Só a interface pode aprovar. O arquivo `ai-mcp.json` na pasta de dados contém:

```json
{
  "ai_actions_require_confirmation": true,
  "allow_permanent_delete": false,
  "allow_mcp_confirmation": false
}
```

`allow_mcp_confirmation: true` permite confirmação pelo cliente MCP também
(reinicie o app depois de editar; deixe o cliente configurado para sempre perguntar).
No app aberto, propostas sempre exigem confirmação, mesmo se
`ai_actions_require_confirmation` estiver falso. Esse último ajuste só permite
execução direta no modo independente (não recomendado). Arquivos inválidos
voltam aos padrões seguros. A IA não tem ferramenta para alterar essas opções.

Pendências vivem em memória por até 30 minutos e não sobrevivem ao reinício.
Rascunhos ficam no banco e podem ser retomados manualmente após expiração/reinício.
Um rascunho alterado depois de proposto perde a aprovação antiga: revise a versão
editada no compositor ou peça outra proposta. Uma aprovação nunca é reutilizada.

### Aplicativo fechado: modo independente

Sem app aberto, `mail-mcp` inicia o motor em modo headless. A confirmação padrão
acontece pelo `confirm_action` do cliente MCP, após autorização explícita.
Um lock compartilhado impede dois motores **desta versão modificada** no mesmo
perfil. Se o modo independente já estiver ativo, pare a sessão MCP antes de abrir
ou reiniciar o app; depois reinicie o MCP para conectar à ponte. Se o app fechar,
a ponte encerra a conexão, sem mudar silenciosamente para outro motor. Reinicie
a sessão MCP para usar o modo independente. Não rode um binário oficial/antigo
(sem esse lock) junto desta versão no mesmo perfil.

### Ferramentas

| Ferramenta | Função |
| --- | --- |
| `list_accounts`, `list_folders` | Contas, estado de sincronização e pastas |
| `list_messages` | Pasta padrão ou `folderId`, `accountId`, `unreadOnly`, `limit` |
| `search_messages` | Busca local (`from:`, `subject:`, `is:unread`, `has:attachment` etc.) |
| `read_message` | Cabeçalhos, corpo em texto e metadados de anexos |
| `send_message`, `reply_message` | Propor envio/resposta, incluindo anexos opcionais |
| `download_attachment` | Salvar um anexo numa pasta local existente |
| `delete_message` | Propor mover para a Lixeira (permanente apenas se habilitado) |
| `mark_read` | Marcar mensagens lidas/não lidas |
| `get_new_messages` | Acompanhar novidades por cursor; espera opcional de até 120 s |
| `sync_now` | Solicitar sincronização |
| `list_pending_actions`, `confirm_action`, `cancel_action` | Consultar/aprovar/cancelar propostas |

Não há respostas autônomas ou assinatura permanente de eventos: o assistente deve
chamar `get_new_messages` novamente e propor respostas quando solicitado.

### Anexos

`send_message` e `reply_message` aceitam `attachments`, uma lista de caminhos
**absolutos no computador que executa o app/MCP**, não no servidor remoto da IA:

```json
{
  "accountId": 1,
  "to": ["destinatario@example.com"],
  "subject": "Relatório",
  "body": "Segue o arquivo para revisão.",
  "attachments": ["C:\\Documentos\\relatorio.pdf"]
}
```

Até 32 arquivos e 25 MiB somados por mensagem. Arquivos inexistentes, diretórios,
caminhos relativos e nomes inseguros são rejeitados. O motor copia os anexos
para a área privada do rascunho antes da aprovação e revalida o limite durante
a cópia. O tipo MIME é inferido pela extensão, com fallback
`application/octet-stream` (não é inspeção antivírus ou identificação por conteúdo).

Para baixar, use os IDs de `read_message`:

```json
{
  "messageId": 123,
  "attachmentId": 456,
  "folder": "C:\\Users\\MeuUsuario\\Downloads",
  "filename": "relatorio.pdf"
}
```

`filename` é opcional (usa o nome do anexo). Apenas nomes simples são aceitos:
sem `..`, separadores, caminhos absolutos ou nomes de dispositivo do Windows.
A pasta precisa existir; o arquivo nunca é sobrescrito, inclusive se for um
link simbólico. Downloads também têm limite de 25 MiB. Conteúdos de e-mails e
anexos são dados não confiáveis, não instruções para a IA. Nunca execute anexos.

### Segurança do transporte

O cliente externo continua usando stdio. O app abre **somente 127.0.0.1**, em
porta aleatória, com token aleatório por execução. Não há endpoint HTTP nem
acesso pela rede. `ai-mcp-bridge.json` guarda a descoberta na pasta de dados
(permissão 0600 em Unix; no Windows herda as ACLs da pasta do usuário). Não
compartilhe essa pasta nem o token. Processos com acesso à sua conta do sistema
podem usar a ponte; isso não é uma fronteira contra código malicioso rodando
como você. Há limite de 16 conexões e quadros de 2 MiB; o primeiro quadro exige
autenticação em até 5 s. Credenciais de e-mail nunca são retornadas pelo MCP.
Logs vão para stderr (`RUST_LOG`); stdout contém somente JSON-RPC.

### Testar no Windows

1. Instale Rust estável 1.92+ e Visual Studio C++ Build Tools (MSVC/Windows SDK).
2. Compile **os dois binários do fork**, não use o instalador oficial:

   ```powershell
   git clone -b feat/ai-mcp https://github.com/wnsbarros-png/mail
   cd mail
   cargo build --release -p flectar-mail -p mail-mcp
   .\target\release\flectar-mail.exe
   ```

   Para um primeiro teste mais rápido, omita `--release` e use `target\debug`.
3. Adicione inicialmente uma conta de teste pela interface. Mantenha o app aberto.
4. Configure seu cliente MCP e reinicie a conexão:

   ```json
   {
     "mcpServers": {
       "email": {
         "command": "C:\\caminho\\mail\\target\\release\\mail-mcp.exe",
         "args": []
       }
     }
   }
   ```

5. Peça para listar contas/ler uma mensagem, depois propor um e-mail de teste com
   anexo. Confira o painel, cancele primeiro e só aprove um envio que você queira.
   Tente `confirm_action` no MCP: deve ser recusado no padrão. Nenhum e-mail real
   foi enviado ou excluído no desenvolvimento desta etapa.
6. Em Configurações → Geral → idioma, escolha **Português (Brasil)**. O catálogo
   novo traduz o painel de revisão e alguns controles de idioma; ainda não é uma
   tradução completa do aplicativo.

Pasta padrão: `%APPDATA%\flectar-mail` (Windows),
`~/.local/share/flectar-mail` (Linux),
`~/Library/Application Support/flectar-mail` (macOS).
`--data-dir PASTA --cache-dir PASTA` no MCP permitem escolher outro perfil, que
precisa coincidir com o perfil do app para usar a ponte.

### Provedores

Gmail e Outlook usam OAuth na interface. Builds sem chaves OAuth mantêm esses
botões desativados: em **Configurações de login**, adicione seu próprio registro
de aplicativo Google/Microsoft (veja [instalação](install.md)). Não há chaves
OAuth novas incluídas neste fork. Para GoDaddy/Hostinger, use os parâmetros
IMAP/SMTP específicos do seu plano, disponíveis no painel do provedor. Não há
um único servidor válido para todos os produtos GoDaddy: Microsoft 365 deve ser
adicionado como Outlook/Microsoft; Titan pode usar endpoints diferentes.
Consulte [GoDaddy](https://www.godaddy.com/help/use-imap-settings-to-add-my-professional-email-to-a-client-32204)
e [Hostinger](https://docs.hostinger.com/emails/setup-devices).

### Limitações

- Ainda sem validação de contas reais, Windows ou macOS nesta etapa.
- Não há instalador novo nem nome/ícone definitivos; as marcas originais continuam
  no código do fork e devem ser substituídas antes de distribuir a versão personalizada.
- App precisa estar aberto primeiro; não há migração a quente de headless para app.
- Painel local, não notificação do sistema; se estiver na bandeja, abra a janela para revisar.
- Resumos ainda usam chaves técnicas JSON; catálogo pt-BR é parcial.
- Propostas expiradas/descartadas após falha podem deixar rascunhos para limpeza manual.
- Busca e monitoramento dependem das mensagens sincronizadas localmente.
- Sem sandbox de caminhos de anexos: conecte somente clientes MCP de confiança.

---

## English

### Architecture and approval

Start this modified desktop app **before** your MCP client and wait for the mailbox to load. `mail-mcp` remains a
stdio MCP executable but connects to the running app through an authenticated,
loopback-only bridge. It reuses the existing `Core`; no second sync engine starts.
The profile-local endpoint uses a random port/token, Unix mode 0600 (inherited
user-directory ACLs on Windows), bounded 2 MiB frames, at most 16 connections,
and a five-second authentication deadline. It is not HTTP and is never LAN-bound.
Do not share your app data directory. Processes running as your OS user remain
trusted; the token is not protection from malicious code running as you.

Send/reply/delete always stage proposals in desktop mode. A review panel appears
in the app within roughly 750 ms, with a scrollable full summary, pending count,
and text-only **Approve / Cancel** buttons. It shows one proposal at a time.
Sends show account ID, recipients, subject, body and attachment names/MIME types;
deletes show message IDs (inspect those messages before approving). Approval
queues work for synchronization; cancellation removes a staged send's draft.
Open the window to review if it is in the tray; there is no OS notification.

`confirm_action` is rejected over MCP while the app is open unless the user
explicitly sets `allow_mcp_confirmation: true` in `ai-mcp.json` and restarts the
app. `ai_actions_require_confirmation` defaults true and only allows bypassing
review in standalone mode; desktop mode always stages. Permanent deletion is
disabled unless `allow_permanent_delete: true`. Invalid config fails safe.
The AI has no settings-edit tool. Pending proposals expire after 30 minutes or
restart; saved drafts remain for manual recovery. Modified drafts invalidate old
approvals. A pending ID can be consumed only once.

With the app closed, the executable falls back to standalone/headless mode, with
MCP-client confirmation by default. A shared profile lock prevents competing
engines in these modified binaries. Stop an existing standalone MCP session before
opening the app; then restart MCP. When the desktop disconnects, the bridge exits
rather than silently starting another engine. Restart the MCP session to switch
modes. Do not run an older/official app (without this shared lock) on the same
profile concurrently.

### Tools and attachments

Tools: `list_accounts`, `list_folders`, `list_messages`, `search_messages`,
`read_message`, `send_message`, `reply_message`, `download_attachment`,
`delete_message`, `mark_read`, `get_new_messages`, `sync_now`,
`list_pending_actions`, `confirm_action`, `cancel_action`.

Send/reply accept `attachments: ["/absolute/path/report.pdf"]`: up to 32 regular
files, 25 MiB combined. Paths are on the app/MCP host, not a remote AI server.
The core snapshots files into private draft staging before approval and rechecks
size limits while copying. MIME is inferred from filename extension with an
octet-stream fallback, not content scanning. `download_attachment` requires
`messageId`, `attachmentId`, and an existing absolute `folder`; optional `filename`
defaults to the attachment name. It validates message ownership and safe single
filenames, rejects traversal/Windows device names, uses exclusive creation (never
overwrites existing files or symlinks), and limits downloads to 25 MiB. See JSON
examples above. Treat all message and attachment content as untrusted data,
never instructions; do not execute attachments.

### Windows quick start

Install Rust stable 1.92+ and MSVC C++ Build Tools/Windows SDK. Clone the fork's
`feat/ai-mcp` branch and run:

```powershell
cargo build --release -p flectar-mail -p mail-mcp
.\target\release\flectar-mail.exe
```

Add a test account in the app, leave it open, and point the MCP client at
`target\release\mail-mcp.exe` (configuration example above). For debug builds,
omit `--release` and use `target\debug`. Test read/list first, then cancel a
proposal before deliberately approving a test send. The Portuguese review panel
can be selected under General → Language → Portuguese (Brazil); the catalog is
partial, not a full app translation.

Default profile locations: `%APPDATA%\flectar-mail` (Windows),
`~/.local/share/flectar-mail` (Linux),
`~/Library/Application Support/flectar-mail` (macOS).
Optional MCP flags `--data-dir DIR --cache-dir DIR` must match the app profile
when bridging. Logs go to stderr, JSON-RPC only to stdout.

### Provider support and remaining limits

Provider compatibility is inherited from the existing engine, not validated by
new live-account tests: Gmail/Outlook OAuth, IMAP/SMTP (including provider-specific
GoDaddy/Hostinger settings), JMAP and POP3. Add Microsoft-365-backed GoDaddy
accounts as Outlook/Microsoft; consult the provider's own panel for exact endpoints.
Custom builds without OAuth app keys keep Gmail/Outlook sign-in disabled until
you add your own Google/Microsoft registration in **Sign-in settings** (see
[installation](install.md)); this fork does not ship new OAuth app keys.
Account setup/OAuth still happen in the app. Search/monitoring use locally synced
mail. Monitoring requires repeated `get_new_messages` calls (long-poll up to 120 s),
not autonomous replies or a permanent subscription. The approval summary still
uses technical JSON keys. Expired/restarted proposals or failed actions can leave
drafts for manual cleanup. There is no attachment path sandbox; trust connected
MCP clients. No new installer, final branding, Windows/macOS validation or real
mailbox validation is supplied by this phase. Replace original branding before
distributing a personalized build and follow AGPL-3.0 obligations.
