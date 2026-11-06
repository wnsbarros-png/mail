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
   proposta por vez, o total pendente e um resumo legível em pt-BR em área rolável:
   remetente, destinatários, assunto, prévia do corpo e anexos para envios;
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
**Exceção: uploads de arquivos como link exigem sempre aprovação no app, mesmo com essa opção.**
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

Para anexos MIME normais: até 32 arquivos e 25 MiB somados por mensagem.
Para arquivos maiores, veja **Arquivos grandes como links** abaixo. Arquivos inexistentes, diretórios,
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

### Arquivos grandes como links (Google Drive, OneDrive, Dropbox)

**O aplicativo precisa estar aberto.** Selecione arquivos no compositor (até
20 no seletor da interface; até 32 via MCP; no máximo 2 GiB somados). Ao exceder
25 MiB, a interface oferece **Enviar como link: Google Drive / OneDrive / Dropbox**;
também é possível escolher links para arquivos menores. Informe a conta de
armazenamento Google/Microsoft pelo ID local, que pode ser diferente da conta
remetente (veja `list_accounts`), e, se desejado, uma expiração RFC3339.

**Configurações de login → Configurações de login na nuvem** e o botão equivalente
no compositor permitem autorizar os provedores. Primeiro adicione uma conta de
email; Drive exige uma conta Gmail e OneDrive uma conta Microsoft já existente.
Dropbox usa uma conexão própria/global, independente do remetente. Não há chaves
nem contas de armazenamento fornecidas pelo fork.

1. Criar a proposta apenas copia os arquivos para `cloud-pending/review-*` dentro
   do perfil. Nenhum arquivo é enviado à nuvem. Uma cópia privada e seu SHA-256
   fixam os bytes revisados, mesmo que o arquivo original seja alterado.
2. O painel mostra De/Para/Cc/Cco, assunto, prévia do corpo, arquivos/tamanhos/
   caminhos de origem, provedor/conta e expiração. O aviso deixa explícito:
   **qualquer pessoa com o link poderá baixar**. O painel e os botões não executam HTML.
3. **Cancelar** remove as cópias privadas e o rascunho, sem upload. **Aprovar**
   resolve as credenciais no chaveiro, verifica o rascunho/arquivos e só então
   cria as sessões de upload. O progresso por arquivo aparece no painel.
4. Após todos os uploads e permissões públicas de leitura serem confirmados,
   o rascunho recebe um bloco limpo em **texto e HTML** com nome, bytes,
   provedor, URL e expiração confirmada (ou “sem expiração”). O email é colocado
   na fila do motor de email; os arquivos não são anexados ao MIME.
5. Falhas de upload/permissão/expiração não enfileiram email. Tentamos remover
   apenas os novos objetos criados pela própria ação. Um erro de rede, queda do
   processo ou falha de limpeza pode deixar sessões/objetos órfãos; confira o
   armazenamento antes de propor novamente. Se salvar/enfileirar falhar depois
   de gerar os links, eles podem continuar ativos para recuperação manual.

`send_message` e `reply_message` aceitam as opções **snake_case** abaixo, além
dos parâmetros existentes; `reply_message` usa `messageId` em vez dos campos de
nova mensagem:

```json
{
  "accountId": 1,
  "to": ["destinatario@example.invalid"],
  "subject": "Arquivo grande",
  "body": "Veja o arquivo para baixar abaixo.",
  "attachments": ["C:\\Documentos\\video.mp4"],
  "attachments_as_link": true,
  "provider": "google_drive",
  "storage_account_id": 2
}
```

`provider`: `google_drive`, `one_drive` (ou `onedrive`), `dropbox`.
`storage_account_id` identifica a conta de armazenamento, não o remetente;
Dropbox ignora esse ID. `expires_at` é opcional, RFC3339 futuro, somente para
OneDrive/Dropbox quando o servidor realmente confirmar a expiração. Não há
fallback silencioso para link restrito à organização, de edição ou sem a
expiração solicitada. `confirm_action` **nunca** aprova upload de nuvem:
a confirmação deve ser pelo botão do aplicativo. Headless não aceita essas propostas.

Arquivo opcional `cloud-links.json` na pasta do perfil (a UI salva apenas a
chave pública Dropbox; os demais ajustes podem ser editados neste arquivo):

```json
{
  "account_limits": { "1": 20971520 },
  "auto_link_over_limit": false,
  "default_provider": "google_drive",
  "storage_account_id": 2,
  "dropbox_app_key": "SUA_CHAVE_PUBLICA"
}
```

O limite padrão é **25 MiB somados**. Pode ser reduzido por conta em bytes;
valores acima de 25 MiB são limitados ao teto MIME atual do motor. Acima desse
limite, o MCP pede escolha explícita de links. Se você habilitar
`auto_link_over_limit`, o provedor padrão é escolhido automaticamente, **mas o
upload continua exigindo aprovação no aplicativo**. Configuração inválida
volta ao padrão seguro. O limite da seleção de links é 2 GiB, independente do
plano/cota do provedor. Não há progresso do estágio de cópia local nem cancelamento
no meio de um upload já aprovado; as sessões não são retomadas após erro/reinício.

#### Provedores e OAuth

| Provedor | Upload e compartilhamento | Permissão / restrições |
| --- | --- | --- |
| Google Drive | Sessão resumable; blocos 5 MiB; progresso; `anyone/reader`; exclusão do novo objeto na limpeza | Habilite Drive API no mesmo projeto OAuth e autorize o escopo incremental **`https://www.googleapis.com/auth/drive.file`**. Só arquivos criados por este aplicativo. Links `anyone` do Drive **não aceitam expiração**. |
| OneDrive pessoal/empresa | Upload sessions; blocos 5 MiB (múltiplos de 320 KiB); progresso; `createLink` `anonymous/view`; limpeza por ID | No registro Microsoft existente permita o escopo delegado **`Files.ReadWrite`** do Graph (`https://graph.microsoft.com/Files.ReadWrite`) e `offline_access`, além dos escopos de email existentes. Políticas do tenant podem proibir links anônimos; expiração depende de conta/política e precisa ser confirmada pelo servidor. A URL de upload pré-autorizada nunca recebe o bearer Graph. |
| Dropbox | Sessões start/append/finish; blocos 5 MiB; progresso; público/viewer; limpeza por ID | App próprio com PKCE S256/offline, escopos **`files.content.write sharing.write`**. Expiração pode exigir plano pago; restrições de equipe são respeitadas com falha segura. |

“Resumable” descreve o protocolo usado para enviar blocos e verificar offsets;
esta versão **não faz retry automático nem persiste sessões para retomar após
falhas/reinícios**. Não confundir com uma fila de upload durável. Tokens são
salvos somente no chaveiro do SO; logs não incluem tokens, respostas OAuth ou
URLs de sessão pré-autorizadas. Arquivos são criados com nomes únicos, sem
sobrescrever objetos preexistentes. A revogação manual da URL é feita no site
do provedor; a interface não tem botão de revogar links após o envio.

**Criar chave Dropbox:** abra [Dropbox App Console](https://www.dropbox.com/developers/apps),
Create app → Scoped access → **App folder** (preferível ao acesso total), escolha
um nome. Em Permissions marque `files.content.write` e `sharing.write`, salve.
Em Settings registre exatamente **`http://127.0.0.1:53682/`** como OAuth redirect
URI. O aplicativo usa PKCE S256 (não exige App secret). Copie apenas **App key**, nunca App secret,
para Configurações de login na nuvem → Salvar chave → Conectar Dropbox. Conclua
o consentimento no navegador. A porta precisa estar livre; o fluxo expira em
cinco minutos. Tokens de atualização ficam no chaveiro e não no JSON.

**MEGA adiado, não simulado:** o crate Rust não oficial
[`mega` / mega-rs](https://github.com/Hirevo/mega-rs) ainda lista criar links
públicos de nós próprios como não implementado; a última versão encontrada é
0.8 (2024). O [SDK oficial](https://github.com/meganz/sdk) é C++ e exige integrar
FFI, criptografia ponta a ponta e empacotamento de maneira auditável. Isso não
é uma implementação segura rápida em Rust. A trait `StorageProvider` separa
upload/progresso, share e delete para integração futura. WebDAV genérico só
padroniza PUT, não links públicos; Nextcloud exigiria integração OCS e
credenciais próprias, também adiada. Não implemente criptografia MEGA artesanal.

**Retenção e segurança:** pendências duram 30 minutos/até o app fechar. Cópias
privadas são removidas em cancelamento/expiração/sucesso/erro normais. Uma queda
abrupta pode deixar `cloud-pending` no perfil; feche app/MCP antes de limpar essa
pasta manualmente. Não há antivírus ou sandbox de caminhos. Links enviados são
públicos por posse e podem ser encaminhados; evite dados confidenciais.

**Teste no Windows:** após compilar ambos os binários e configurar uma conta de
teste, autorize o armazenamento nas configurações acima; selecione um arquivo
maior que 25 MiB, escolha o provedor, clique Enviar e confira o resumo. Cancele
primeiro e confirme no provedor que não houve upload. Só aprove outro teste se
quiser realmente criar um link público e enviar email. Desenvolvimento/testes
desta fase usam somente dados sintéticos/mocks; nenhum upload ou email real.

![Painel de aprovação com dados sintéticos, sem upload ou envio real](images/ai-cloud-approval-pt-BR.png)

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

### Validação desta fase

Builds debug completos do desktop e `mail-mcp` em Linux, Rust 1.99.1.
`cargo test -p flectar-mail-core -p mail-mcp`: **986 core + 29 MCP aprovados**;
2 testes Stalwart ignorados. `cargo test -p flectar-mail`: **442 unitários +
30 de interface/integração aprovados**, 7 ignorados. O teste ignorado de captura
do painel foi executado separadamente e passou. `cargo fmt --all --check` e
`cargo clippy -p flectar-mail-core -p mail-mcp -p flectar-mail --all-targets -- -D warnings`
passaram (também verificado rustfmt direto nos arquivos incluídos por `src/lib.rs`).
Os novos provedores usam mocks HTTP locais; suítes dependentes de servidores
externos não configurados não validam esses servidores. Sem contas reais,
uploads/envios reais, build release ou validação Windows/macOS.

### Limitações

- Ainda sem validação de contas reais, Windows ou macOS nesta etapa.
- Não há instalador novo nem nome/ícone definitivos; as marcas originais continuam
  no código do fork e devem ser substituídas antes de distribuir a versão personalizada.
- App precisa estar aberto primeiro; não há migração a quente de headless para app.
- Notificação do sistema é genérica e depende das permissões/serviço de notificações do SO; abra a janela para decidir.
- Resumos de aprovação são legíveis em pt-BR; o restante do catálogo pt-BR ainda é parcial.
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
in the app within roughly 750 ms, with a readable Portuguese summary/body preview, pending count,
and text-only **Approve / Cancel** buttons. It shows one proposal at a time.
Sends show account ID, recipients, subject, body and attachment names/MIME types;
deletes show message IDs (inspect those messages before approving). Approval
queues work for synchronization; cancellation removes a staged send's draft.
A generic, deduplicated OS notification announces new proposals (no mail content or links).
It respects notification settings; platform notification permissions/services are required.
Open the window to review if it is in the tray.

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
files, 25 MiB combined for ordinary MIME attachments (cloud links: see below). Paths are on the app/MCP host, not a remote AI server.
The core snapshots files into private draft staging before approval and rechecks
size limits while copying. MIME is inferred from filename extension with an
octet-stream fallback, not content scanning. `download_attachment` requires
`messageId`, `attachmentId`, and an existing absolute `folder`; optional `filename`
defaults to the attachment name. It validates message ownership and safe single
filenames, rejects traversal/Windows device names, uses exclusive creation (never
overwrites existing files or symlinks), and limits downloads to 25 MiB. See JSON
examples above. Treat all message and attachment content as untrusted data,
never instructions; do not execute attachments.

### Large attachments as cloud links

The desktop must be running. Compose offers **Send as link: Google Drive /
OneDrive / Dropbox** for attachments over the account limit, and optionally for
smaller files. Choose the storage account by local ID (`list_accounts`); it may
differ from the sending account. UI picker supports 20 files, MCP 32, cloud
selection at most 2 GiB combined. Ordinary MIME remains capped at 25 MiB.

In **Sign-in settings → Cloud sign-in settings** (also reachable from Compose),
connect an existing Gmail account for Drive, an existing Microsoft account for
OneDrive, or an independent/global Dropbox connection. The app supplies no new
OAuth registrations or storage accounts.

MCP send/reply accept `attachments_as_link: true`, `provider: "google_drive" |
"one_drive" | "dropbox"`, `storage_account_id` and optional future RFC3339
`expires_at` (not supported for Drive anyone-links). Existing arguments remain
camelCase. See the valid JSON example above. These proposals are **always
app-approved**, even if `allow_mcp_confirmation` is enabled. Standalone refuses
them rather than letting the AI authorize public uploads.

Staging makes bounded private copies with SHA-256; no attachment is uploaded.
The readable Portuguese panel lists From/To/Cc/Bcc, subject, body preview,
filenames/sizes/source paths, provider/account, expiry and an explicit
**anyone-with-the-link can download** warning. Cancel deletes the staged copies
and draft. Only **Approve in the app** resolves keyring credentials, verifies
snapshots and creates upload sessions. Per-file acknowledged progress appears
in the review panel. All successful share links are inserted into **plain text
and HTML** with escaped name, size, provider, URL and confirmed expiry before
mail is queued. No large MIME payload is attached.

Permission/upload/expiry failures do not queue mail. Best-effort cleanup deletes
only objects newly created by the approved action. Partial sessions/orphan files
can remain after network/process/cleanup failures; inspect storage before retrying.
If final draft-save/queueing fails after sharing, live links may remain for manual
recovery. Cancellation is not supported mid-upload; retry is a new proposal.

Optional profile `cloud-links.json` (example above): `account_limits` in bytes
(default 25 MiB, configurable downward to the current engine MIME ceiling),
`auto_link_over_limit` **false by default**, `default_provider`,
`storage_account_id`, public `dropbox_app_key`. Auto fallback only chooses link
mode; it never skips app approval. Invalid JSON fails safe. The UI edits the
Dropbox public key, not the other JSON settings. No copy-stage progress yet.

| Provider | Implemented | OAuth / limits |
| --- | --- | --- |
| Google Drive | Resumable REST upload, 5 MiB chunks, progress, anyone/reader permission, cleanup/delete | Reuses Gmail OAuth with incremental **`https://www.googleapis.com/auth/drive.file`**; enable Drive API in that OAuth project. Only app-created files. Public anyone-links cannot have expiry. |
| OneDrive personal/business | Upload sessions, 5 MiB chunks (320 KiB aligned), progress, anonymous/view `createLink`, cleanup/delete | Reuses Microsoft OAuth; Graph delegated **`Files.ReadWrite`** (`https://graph.microsoft.com/Files.ReadWrite`) plus `offline_access` and existing mail scopes. Tenant policy may deny anonymous links; expiry must be confirmed by the server. No Graph bearer on preauthenticated upload PUTs. |
| Dropbox | Upload-session start/append/finish, 5 MiB chunks, progress, public/viewer links, cleanup/delete | Independent PKCE S256/offline public app; **`files.content.write sharing.write`**. Expiry may require a paid plan. Team restrictions fail closed. |

All providers use resumable/session protocols but **there is no automatic retry
or persisted cross-error/restart resume** yet. Do not treat this as a durable
upload queue. New unique remote names never overwrite existing files. Tokens
are OS-keyring-only, never logged; OAuth responses/session bearer URLs are not
logged. Post-send manual revocation is via the provider website, not an app button.

**Dropbox setup:** in [App Console](https://www.dropbox.com/developers/apps),
Create app → Scoped access → **App folder** (least privilege), name it; Permissions:
enable `files.content.write`, `sharing.write`, save. Settings: register exactly
**`http://127.0.0.1:53682/`** as OAuth redirect URI. This app uses PKCE S256 without an App secret.
Copy **App key**, not App secret, into Cloud sign-in settings, Save, Connect
Dropbox and authorize in the browser. The loopback port must be free; consent
times out after five minutes. Refresh tokens go only into the OS keyring.

**MEGA deferred:** unofficial Rust [`mega-rs`](https://github.com/Hirevo/mega-rs)
still marks public-link creation for owned nodes unimplemented (latest found:
0.8, 2024). The maintained [official SDK](https://github.com/meganz/sdk) is C++:
correct E2E crypto, FFI and packaging need a separate audited integration, not
hand-written cryptography. `StorageProvider` keeps upload/progress/share/delete
pluggable for later. Generic WebDAV does not standardize public-share links;
Nextcloud needs its own OCS/credentials integration and is also deferred.

Staged proposals expire after 30 minutes or app shutdown. Private copies are
removed on normal cancel/expiry/success/error; abrupt crashes can leave
`cloud-pending`. Stop both app and MCP before manually clearing that folder.
No antivirus or attachment path sandbox. Anyone can forward a public link;
avoid confidential files. Windows/macOS and live provider accounts were not
validated: tests use synthetic files and mocked HTTP, never real mail/uploads.

For a Windows trial, build both binaries as below, connect a test storage account
in Cloud sign-in settings, pick a >25 MiB file and its provider, Send, then inspect
the review. Cancel first and verify no file was uploaded. Only approve another
proposal if you deliberately want a real public upload and email send.

### API references / Referências

- [Drive uploads](https://developers.google.com/workspace/drive/api/guides/manage-uploads),
  [drive.file scope](https://developers.google.com/workspace/drive/api/guides/api-specific-auth),
  [permission expiration restrictions](https://developers.google.com/workspace/drive/api/reference/rest/v3/permissions).
- [OneDrive upload sessions](https://learn.microsoft.com/en-us/graph/api/driveitem-createuploadsession?view=graph-rest-1.0),
  [anonymous view links](https://learn.microsoft.com/en-us/graph/api/driveitem-createlink?view=graph-rest-1.0).
- [Dropbox OAuth/PKCE](https://developers.dropbox.com/oauth-guide),
  [Dropbox HTTP API](https://www.dropbox.com/developers/documentation/http/documentation).

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

### Validation for this phase

Full Linux debug desktop + MCP builds, Rust 1.99.1. Core/MCP suites: **986 core +
29 MCP passing**, 2 Stalwart tests ignored. Full desktop suite: **442 unit +
30 integration/UI passing**, 7 ignored; the ignored synthetic screenshot test
was explicitly run separately and passed. Formatting checks (including direct
rustfmt for files included via `src/lib.rs`) and all-targets clippy **`-D warnings`**
passed for all three packages. New storage tests use mocked local HTTP.
Environment-gated external-server tests do not validate unconfigured servers.
No live accounts/uploads/emails, release build, or Windows/macOS validation.

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
not autonomous replies or a permanent subscription. The approval panel now uses readable Portuguese text, not technical JSON. Expired/restarted proposals or failed actions can leave
drafts for manual cleanup. There is no attachment path sandbox; trust connected
MCP clients. No new installer, final branding, Windows/macOS validation or real
mailbox validation is supplied by this phase. Replace original branding before
distributing a personalized build and follow AGPL-3.0 obligations.
