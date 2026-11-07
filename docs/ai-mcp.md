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
4. Por padrão, enviar, responder e excluir criam propostas (as regras abaixo podem permitir ou bloquear). Em até aproximadamente 750 ms,
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
`ai_actions_require_confirmation` e `allow_permanent_delete` são opções legadas: agora **`ai-policy.json` governa as ações em ambos os modos**, sem migração automática para permissões antigas. Os padrões continuam seguros; regras explícitas podem permitir ações normais sem perguntar. Exclusão permanente tem padrão Bloquear. A IA não tem ferramenta para alterar regras ou opções.

Pendências vivem em memória por até 30 minutos e não sobrevivem ao reinício.
Rascunhos ficam no banco e podem ser retomados manualmente após expiração/reinício.
Um rascunho alterado depois de proposto perde a aprovação antiga: revise a versão
editada no compositor ou peça outra proposta. Uma aprovação nunca é reutilizada.

### Regras por ação e histórico (novo)

Abra **Configurações → Geral → Regras de aprovação e histórico da IA**. O mesmo botão está nas configurações de login/nuvem. O motor do perfil precisa estar carregado. Há opções **Sempre permitir / Perguntar / Bloquear** para listar, ler/monitorar, pesquisar, marcar leitura, enviar, responder, mover à lixeira, excluir permanentemente, upload na nuvem, baixar anexos e sincronizar. Botões não têm emojis. Os rótulos da tela têm traduções pt-BR/EN; mensagens operacionais e histórico são bilíngues.

- **Padrões seguros:** listar/ler/pesquisar permitidos; enviar/responder/lixeira/upload, marcar leitura, baixar e sincronizar perguntam; exclusão permanente bloqueada. As ações manuais normais do compositor não são ações de IA; envio manual como link passa pela mesma política de upload. Esta política governa o MCP e a ponte, não todas as funções internas de classificação por IA do projeto original.
- Todas as ferramentas passam pelo mesmo avaliador. Respostas incluem `policy.effect`, `applied` (ação/ID da regra/efeito), `reason`, `revision`, `safe_fallback` e `in_app_only`. `read_message(markRead:true)` também avalia Marcar leitura; responder também avalia Ler. Um Bloquear em qualquer parte bloqueia a operação inteira. Pedidos multi-conta são negados por inteiro quando uma conta corresponde a um bloqueio; não há filtragem parcial silenciosa. Em operações de mensagem, usam-se os destinatários disponíveis no cabeçalho (To/Cc; também Bcc para envio). Em listagens/buscas sem metadados de cada mensagem, condições desconhecidas não habilitam Permitir e são tratadas conservadoramente como correspondentes para Bloquear/Perguntar: a operação inteira é bloqueada/perguntada, sem filtragem por linha.
- Regras têm ID e condições opcionais: IDs das contas, todos os destinatários serem contatos **salvos**, lista de endereços/domínios exatos, conter destinatários novos, mínimo/máximo de destinatários únicos, anexos, links na nuvem. To/Cc/Bcc são considerados e deduplicados. A condição de links identifica anexos solicitados como link, não procura URLs arbitrárias no corpo. Contatos aprendidos apenas de e-mails recebidos **não** autorizam novos destinatários; salvos ou com `send_count > 0` contam como já conhecidos para a proteção de primeiro envio. Desativar aprendizagem de contatos pode fazer a proteção perguntar novamente.
- Todas as condições são combinadas com E. Endereços e domínios formam uma lista combinada (OU). Para Permitir, **todos** os destinatários/contas precisam estar cobertos; para Bloquear/Perguntar, basta **qualquer** destinatário/conta dessa lista. Domínios são exatos, ignorando caixa, sem curinga ou correspondência por sufixo. Um Bloquear aplicável prevalece sobre todas as outras regras; caso contrário, a primeira regra aplicável vence, depois o padrão da ação.
- **Invariantes:** upload na nuvem nunca se torna execução direta: exige aprovação no aplicativo mesmo com Permitir, e não funciona no modo independente. Enviar/responder a destinatários nunca contatados sempre pergunta, salvo uma regra Permitir **explícita** com `override_new_recipients:true`. Mudar apenas o padrão para Permitir não remove a proteção. Bloquear continua prevalecendo.
- Salvar atualiza `ai-policy.json` versão 1 atomicamente (0600 em Unix), com revisão crescente. Arquivo ausente usa os padrões; JSON/versão/campos/regras inválidos, arquivo grande ou link simbólico usam padrões seguros, sinalizados por `safe_fallback`. A interface permite restaurar padrões. Não há ferramenta MCP para escrever regras. As regras são reavaliadas na aprovação, e antes de enfileirar depois de um upload; não desfazem efeitos já iniciados. O editor mantém a ordem; para reordenar, remova/recrie a regra ou edite o arquivo com o aplicativo parado. Mudar o ID cria uma nova regra, sem renomear/remover a anterior.
- Se Ler/Buscar/Listar estiver em Perguntar, o pedido fica pendente sem revelar os dados. Após Aprovar no aplicativo, chame `get_action_result({"pendingId":"..."})`. A saída fica **somente em memória por até 10 minutos**, limitada a 32 resultados/16 MiB totais, com reavaliação de Bloquear ao recuperar. Pode ser removida antes por pressão do cache ou reinício. `not_available` não prova falha: **não repita envios/exclusões automaticamente**. `list_pending_actions` retorna apenas IDs, ação e política; resumos com corpos/caminhos ficam na interface. Respostas a mensagens com Ler=Perguntar não revelam o original na proposta MCP.
- `ai-history.jsonl` é acrescido, nunca truncado/reescrito pela aplicação. Registra ator (cliente MCP/aprovação no app/configurações/sistema), ação, horário em milissegundos UTC (mostrado no horário local na interface), conta/contagens, presença de anexos/links, regra/revisão/decisão e resultado. **Sem corpos, assuntos, endereços de destinatários, caminhos ou credenciais.** Registrar o início/aprovação é obrigatório antes dos efeitos; falha bloqueia. Se o registro final falhar após um efeito, a saída marca `auditIncomplete` e avisa para não repetir automaticamente. Cancelamento/expiração registram na medida do possível para não impedir o descarte seguro.
- Histórico somente leitura mostra até 200 registros da cauda de 256 KiB; Recarregar atualiza. Não há rotação/limpeza automática, exportação ou proteção criptográfica contra alteração pelo próprio usuário do SO. Registros parciais/corrompidos são ignorados na leitura; o próximo append inicia uma linha nova sem apagar o trecho parcial. O arquivo pode crescer e requer espaço livre.

Exemplo de regra (o restante dos padrões pode ser omitido; usa padrões seguros por ação):

```json
{"version":1,"rules":[{"id":"contatos-sem-anexos","action":"send","effect":"allow","conditions":{"accounts":[1],"known_contacts":true,"domains":["example.invalid"],"max_recipients":3,"has_attachments":false,"has_cloud_links":false},"override_new_recipients":false}]}
```

![Tela real de regras com dados sintéticos](images/ai-policy-settings-pt-BR.png)

**Teste no Windows:** compile os dois executáveis debug conforme abaixo; abra o app primeiro, conecte apenas uma conta de teste e abra Regras de aprovação e histórico. Confirme os padrões; coloque Ler em Perguntar, solicite uma leitura, aprove no app e recupere com `get_action_result`. Teste Bloquear envio e destinatário novo sem realizar envios reais. Revise o histórico. Esta tela foi capturada em Linux sem contas reais; Windows ainda não foi validado.

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
| `list_pending_actions`, `confirm_action`, `cancel_action` | Consultar IDs/metadados, aprovar/cancelar propostas |
| `get_action_result` | Recuperar a saída de uma ação aprovada no aplicativo (cache temporário) |

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
A pasta precisa existir e ficar **fora da pasta de dados do aplicativo**, mesmo com regra Permitir (impede criar arquivos de política/configuração por download). Apelidos por link simbólico também são recusados. O arquivo nunca é sobrescrito, inclusive se for um
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
`cargo test -p flectar-mail-core -p mail-mcp`: **986 core + 58 MCP aprovados**;
2 testes Stalwart ignorados. `cargo test -p flectar-mail`: **444 unitários +
30 de interface/integração aprovados**, 8 ignorados. O teste ignorado da captura
da tela de regras foi executado separadamente e passou. `cargo fmt --all --check` e
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

By default, send/reply/delete stage proposals in desktop mode; explicit policy rules can instead allow or block them. A review panel appears
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
app. Legacy `ai_actions_require_confirmation` / `allow_permanent_delete` no longer control execution: **`ai-policy.json` governs both embedded and standalone requests**. No unsafe legacy settings are migrated automatically. Permanent deletion defaults to Block; explicit policy can change it. Invalid policy files use safe defaults.
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

### Per-action rules and append-only history (new)

Open **Settings → General → AI approval rules and history** (also available under Sign-in/Cloud settings), after the profile engine has loaded. Text-only controls have Portuguese/English labels; operational statuses/history use bilingual text. Select **Always allow / Ask / Block** for each action. Safe defaults allow list/read/search; send/reply/trash/cloud upload, mark-read, download and sync ask; permanent delete is blocked. Normal manual composition is outside AI policy, except manual cloud sends use the same upload boundary. Existing internal AI categorization is not covered.

A single evaluator governs every MCP tool and the bridge. Results/errors identify `policy.effect`, `applied` action/rule IDs/effects, `reason`, `revision`, `safe_fallback` and `in_app_only`. Reading with `markRead:true` also checks mark-read policy; replies also check read policy. Compound operations fail if any facet blocks. Message actions use available header recipients (To/Cc; outgoing sends also include Bcc). Lists/searches do not know every matching message’s metadata: unknown recipient/attachment conditions cannot match Allow and conservatively match Ask/Block, affecting the whole operation rather than filtering rows. Multi-account requests fail wholly on a matching restrictive account rule; no silent partial filtering.

Conditions: account IDs, saved-address-book contacts, exact address/domain allowlists, new recipients, minimum/maximum unique recipient count, attachments and cloud links. To/Cc/Bcc are deduplicated together. The cloud-link condition describes requested cloud attachments, not arbitrary pasted URLs in the body. Saved contacts or addresses with outgoing `send_count > 0` are previously contacted; incoming-only mail/spam cannot bypass the first-recipient guard. Turning off outgoing contact learning may conservatively ask again. Saved-contact rules specifically require address-book entries, not learned suggestions.

Conditions combine with AND; addresses/domains within an allowlist combine with OR. Allow must cover every recipient/account; Ask/Block match any account or allowlist recipient. Domains are exact, case-insensitive, no wildcards/suffix matching. Any matching Block wins globally; otherwise the first matching rule wins, then the per-action default. Cloud **always requires in-app approval**, even with Allow and MCP confirmation enabled. New recipients always ask unless an explicit matching Allow rule sets `override_new_recipients:true`; changing only the default cannot override this. Block still wins.

Versioned `ai-policy.json` saves atomically (Unix 0600), increments revision and falls back safely on invalid JSON/schema/version/rules, oversized/non-regular files or symlinks; missing files use defaults. No MCP settings-write tool exists. Approval rechecks current policy; cloud sends recheck before queueing after upload. Policy changes cannot roll back already-started effects. The UI preserves rule order; remove/recreate or edit offline to reorder. Changing an ID creates a new rule rather than renaming the old one. The JSON example in the Portuguese section is complete and uses safe defaults for omitted actions.

Ask also works for reads/searches/lists: approve in the app, then call `get_action_result({"pendingId":"..."})`. Outputs are **memory-only for at most 10 minutes**, bounded to 32 entries/16 MiB total; eviction/restart can make them unavailable sooner. Retrieving rechecks the underlying Block policy. `not_available` is not proof of failure: **never automatically retry mutations**. Pending-list controls expose IDs/action/policy only, not bodies/paths. Reply proposals do not expose the original content to MCP when read policy asks. Review/confirm/cancel/result-fetch controls have a fixed policy to avoid deadlocking review, but confirmation/result access always recheck the underlying action.

`ai-history.jsonl` is append-only in application behavior: actor, action, timestamp, account IDs/counts, attachment/cloud flags, rule/revision/decision/result; **no bodies, subjects, recipient addresses, file paths or credentials**. Start/approval logging must succeed before effects. If the final log fails after an effect, the result includes `auditIncomplete` plus a do-not-retry warning. Cancel/expiry logging is best effort so safe cleanup remains possible. Policy saves record intent first; a failure after the configuration was persisted explicitly asks you to reload before retrying.

The app shows the latest at most 200 records from the last 256 KiB, in local time, updated with Reload. No automatic rotation/deletion/export, cryptographic integrity or protection against the same OS user editing the log. Partial/corrupt lines are skipped; a future append adds a newline without erasing crash remnants. The file can grow. Screenshot above is the actual Slint screen with synthetic data.

**Windows trial:** build both debug binaries, start the app first, use test accounts only, open Rules, verify defaults, set Read to Ask, approve a read in-app and retrieve its result. Set Send to Block and verify refusal/history without actually sending. Windows remains unvalidated; Linux screenshot/tests use no real accounts/uploads/emails.

### Tools and attachments

Tools: `list_accounts`, `list_folders`, `list_messages`, `search_messages`,
`read_message`, `send_message`, `reply_message`, `download_attachment`,
`delete_message`, `mark_read`, `get_new_messages`, `sync_now`,
`list_pending_actions`, `confirm_action`, `cancel_action`, `get_action_result`.

Send/reply accept `attachments: ["/absolute/path/report.pdf"]`: up to 32 regular
files, 25 MiB combined for ordinary MIME attachments (cloud links: see below). Paths are on the app/MCP host, not a remote AI server.
The core snapshots files into private draft staging before approval and rechecks
size limits while copying. MIME is inferred from filename extension with an
octet-stream fallback, not content scanning. `download_attachment` requires
`messageId`, `attachmentId`, and an existing absolute `folder`; optional `filename`
defaults to the attachment name. It validates message ownership and safe single
filenames, refuses writes inside the canonical app-data profile even under Allow rules (including symlink aliases), rejects traversal/Windows device names, uses exclusive creation (never
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
58 MCP passing** (19 unit + 32 embedded + 7 tools), 2 Stalwart tests ignored. Full desktop suite: **444 unit +
30 integration/UI passing**, 8 ignored; the ignored synthetic rules screenshot test
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

## Resumo do dia e triagem (pt-BR)

Abra **Resumo do dia e triagem** na caixa de mensagens ou em Configurações → Geral.
Categorias: **Urgente**, **Cliente**, **Cobrança e financeiro**, **Nota fiscal**,
**Propaganda e newsletter**, **Outros**. O filtro mostra até 200 mensagens por categoria;
**Abrir mensagem** volta ao leitor normal. Escolha uma categoria e **Salvar categoria**:
por padrão é criada uma proposta no painel de aprovação. Feche o resumo, aprove a
correção no aplicativo e atualize a lista. A correção é lembrada para o remetente
(não diferencia maiúsculas/minúsculas), somente naquela conta, inclusive mensagens futuras.
Não move pastas nem altera rótulos IMAP. A regra **Corrigir categoria de triagem** pode
permitir, perguntar ou bloquear correções MCP/aplicativo; eventos são registrados sem conteúdo.

A classificação funciona **sem chave de IA**: correções manuais prevalecem; depois,
urgência/prazo de hoje, indícios de NF-e (assunto ou nome de anexo XML compatível),
boletos/faturas/pagamentos, `List-Unsubscribe` e rótulos automáticos existentes
Marketing/News/Social/Pitch, contatos salvos ou remetentes com histórico de envio e
palavras-chave em português/inglês. É uma heurística, não prova de urgência, segurança ou
validade fiscal. Não abre XML nem baixa anexos/corpos. Assunto/trecho/remetente são
limitados; valores completos permanecem na mensagem original.

**Horário diário (local)** começa em **08:00**, usa o fuso do computador e funciona
somente enquanto o aplicativo está aberto (inclusive minimizado). Verifica a cada
30 segundos; se abrir depois do horário, gera o resumo do dia atual. Não recupera dias
em que ficou fechado. Uma marca SQLite por data impede duplicação após reinício,
mudança de horário ou relógio voltando: a marca ocorre antes da proposta; falhas/crash
não são repetidos automaticamente naquele dia. O resumo fica na visualização e uma
notificação do sistema, sem assunto ou remetente, avisa que está pronto. A notificação
pode depender das permissões do sistema. A visualização é recalculada com os metadados
locais atuais, não é um arquivo histórico imutável; correspondência ainda não sincronizada
não aparece. Dias usam limites locais; períodos inválidos são rejeitados.

**Enviar resumo para mim** fica **desativado**. Ao ativar, escolha o **ID da conta**
(`list_accounts`); o destino é obtido internamente do endereço dessa conta e não é
editável. O envio usa `send_message`, a mesma política central, histórico e painel de
aprovação. Padrão: perguntar; uma política explicitamente permissiva pode enfileirar o
envio sem outra confirmação, sujeita à proteção para destinatário novo. Nenhum envio é
repetido automaticamente por erro. Alterar configurações não dispara envio imediato.

**IA opcional** fica desativada. Quando ativada, o provedor/modelo da configuração
OpenAI-compatible existente para categorização recebe somente remetente, assunto e
um trecho limitado de até 20 itens ainda classificados como Outros por consulta.
Correções e decisões heurísticas não são sobrescritas. Falta de chave, falha ou resposta
fora do enum conserva a classificação local. Conteúdo de e-mail é dado não confiável;
a chamada não recebe ferramentas e não pode executar instruções da mensagem.

Ferramentas novas (todas passam pela política e pelo histórico sem corpos):

```json
{"name":"get_digest","arguments":{"date":"2026-11-06","accountId":1}}
{"name":"get_digest","arguments":{"start":"2026-11-01","end":"2026-11-06"}}
{"name":"get_triage","arguments":{"date":"2026-11-06","messageId":100}}
{"name":"set_category","arguments":{"messageId":100,"category":"invoice"}}
```

`get_digest` e `get_triage`: padrão hoje, datas inclusivas `YYYY-MM-DD`, até 31 dias,
contagens de até 5000 mensagens recebidas locais (não rascunhos/enviadas), `truncated` explícito,
até 200 detalhes por filtro `category`, `total_messages` e `details_truncated`,
`accountId` opcional e `enhance:true` somente com opt-in salvo. `get_triage` aceita
`messageId`; para mensagem antiga inclua seu dia/período. Respostas trazem categoria,
origem (`reason`), contagens, fuso e a regra aplicada. Padrão de leitura: permitir;
se Perguntar, obtenha a resposta com `get_action_result` após aprovação, em vez de
repetir a chamada. `set_category`: `urgent`, `customer`, `finance`, `invoice`,
`newsletter`, `other`; padrão Perguntar. Preferências versionadas e correções ficam
no SQLite do perfil; configuração inválida retorna a 08:00, sem IA e sem envio.
Não usa rotinas da plataforma nem exige serviço adicional. Captura sintética:
`docs/images/daily-digest-pt-BR.png`.

## Daily digest and triage (English)

Open **Daily digest and triage** in the mailbox or Settings → General. The six
categories are Urgent, Customer, Billing and finance, Invoice, Marketing and newsletter,
and Other. Filter the triage mailbox, open a message in the existing reader, or save a
category correction. Corrections default to an approval proposal: close the digest,
approve in the app's review panel, then refresh. A case-insensitive sender correction
wins for this account, including future messages; it does not move mail or change IMAP
labels. The new Categorize action has independent Allow/Ask/Block policy controls.

No AI key is needed: deterministic PT/EN keyword rules, urgency, NF-e-like XML attachment
names, billing, List-Unsubscribe, existing Marketing/News/Social/Pitch automatic labels,
saved contacts and prior outgoing contact history are used. It never fetches message
bodies or attachments. Classification is not a security, urgency or tax-document guarantee.
The default schedule is **08:00 computer-local time**, checked every 30 seconds while the
app is running/minimized. Opening later catches up today only. A durable per-local-day
claim prevents duplicate notification/email proposals across restarts and clock changes;
it is written before generation, so crashes/failures are not retried automatically that
day. OS notification contains no mail content. The view is recalculated from currently
synced local incoming mail, not an immutable archived snapshot; up to 200 rows per filter.

**Email digest to myself** defaults OFF. Select an account ID; its own stored address is
the sole destination, never an editable recipient. Proposals use the same `send_message`
policy, new-recipient safeguard, history and review panel. Ask is the default; an explicitly
permissive policy may queue without another confirmation. No automatic email retries.
Changing settings does not send immediately. **Optional AI enhancement** also defaults OFF;
when enabled, the existing OpenAI-compatible Categorize configuration receives bounded
sender/subject/snippet metadata for at most 20 otherwise-Other items per request. No tools
are exposed; corrections/heuristic decisions are never replaced; unavailable/invalid AI
responses retain the local category.

MCP: `get_digest` and `get_triage` default to today's local date; accept `date` or inclusive
`start`/`end` (YYYY-MM-DD, 1–31 days), optional `accountId`, and `enhance:true` only with saved
opt-in. `get_triage` additionally accepts `messageId` (include the date/range for old mail).
Both scan up to 5000 incoming/non-draft messages for counts and return at most 200 detail
rows per optional `category` filter, `total_messages`, `details_truncated`, explicit scan
truncation, counts, reason and
policy metadata. Reads default Allow; retrieve an approved Ask result with
`get_action_result`, rather than re-proposing. `set_category` accepts messageId and category
`urgent|customer|finance|invoice|newsletter|other`, defaults Ask, and remembers the sender in
that account. All three tools use the central evaluator and content-free history. Versioned
settings, corrections and daily claims live in profile SQLite. Invalid settings fail back
to 08:00, email OFF, AI OFF. No hosted routines/service, no real accounts needed in tests.
Synthetic screenshot: `docs/images/daily-digest-pt-BR.png`.

Digest detail responses are bounded for the local bridge's 2 MiB frame limit (including
MCP text + structured content). Subjects/snippets are limited to 300 characters; no body
is included. Email contains category counts and the first 200 entries with an explicit
truncation note. Scheduled generation covers the current local calendar day up to that
point, not a rolling 24-hour window; later mail appears on manual refresh. / Os detalhes
são limitados para respeitar os quadros de 2 MiB da ponte. Assunto/trecho: até 300
caracteres, sem corpos. O e-mail contém contagens e até 200 itens, com aviso de limite.
O agendamento resume o dia local atual até aquele momento, não as últimas 24 horas;
mensagens posteriores aparecem ao atualizar manualmente.

Optional AI enhancement has a 30-second total budget per digest; unfinished items retain
local categories. / A IA opcional tem limite total de 30 segundos por resumo; itens não
concluídos mantêm a categoria local.

## Modelos de resposta e lembretes de retorno (pt-BR)

### Modelos editáveis

- Em **Escrever → Modelos**, crie, edite, insira e exclua modelos locais. Cinco exemplos
  claramente editáveis são criados uma única vez: orçamento, segunda via de boleto,
  confirmação de pagamento, agradecimento e retorno educado. Editar/excluir um exemplo
  é respeitado; ele não reaparece na próxima abertura.
- Use `{{nome}}`, `{{valor}}`, `{{data}}`, `{{vencimento}}`, `{{empresa}}`,
  `{{assinatura}}` ou nomes personalizados (letras Unicode, números, `_`, `-`, `.`).
  No campo de variáveis, informe **uma linha `nome=valor` por variável**. O primeiro
  `=` separa nome/valor; valores podem conter `=`. Esse editor é de uma linha por valor;
  o MCP também aceita valores multilinha.
- A assinatura padrão da conta preenche `assinatura`: padrão de nova mensagem para
  envio e padrão de resposta para respostas. Configure os padrões existentes de
  assinatura nas preferências da conta. Não são criadas identidades fictícias.
  Uma variável explícita pode substituir a assinatura. A inserção evita duplicar o
  bloco de assinatura gerenciado pelo editor; assinaturas editadas manualmente são
  preservadas como texto e devem ser revisadas.
- Nomes ausentes aparecem em vermelho na janela de modelos; a inserção permanece
  aberta até preencher os valores. Rascunhos incompletos podem ser salvos, mas a
  interface e a fila central de envio recusam variáveis não preenchidas, inclusive
  em assunto/HTML. Isso é conservador: `{{nome}}` literal em uma citação também
  precisa ser removido/substituído antes do envio.
- Valores são texto, não código/HTML nem expressões; não há expansão recursiva.
  A composição rica escapa o texto. Limites: nome 200 bytes, assunto armazenado
  4 KiB, corpo armazenado 64 KiB, 100 variáveis, 16 KiB por valor e 128 KiB de
  saída renderizada. `list_templates` retorna até 200 metadados, sem os corpos.

### MCP

Todas as operações usam a política central e o histórico sem corpos/credenciais:

```json
{"name":"list_templates","arguments":{}}
{"name":"render_template","arguments":{"template_id":1,"accountId":1,"reply":false,"variables":{"nome":"Cliente de teste","valor":"R$ 100,00","data":"10/11/2026","assinatura":"Equipe de teste"}}}
{"name":"send_message","arguments":{"accountId":1,"to":["cliente@example.invalid"],"template_id":1,"variables":{"nome":"Cliente de teste","valor":"R$ 100,00","data":"10/11/2026","assinatura":"Equipe de teste"}}}
{"name":"reply_message","arguments":{"messageId":123,"template_id":5,"variables":{"nome":"Cliente de teste","assinatura":"Equipe de teste"},"quoteOriginal":false}}
{"name":"set_followup","arguments":{"messageId":123,"days":3,"template_id":5}}
{"name":"list_followups","arguments":{"accountId":1}}
{"name":"cancel_followup","arguments":{"messageId":123}}
```

`render_template` devolve `subject`, `body` (texto) e `missing`; não cria/envia mensagem.
Sem `template_id`, `send_message` continua exigindo assunto/corpo e `reply_message`
exige corpo. Com modelo, assunto/corpo avulsos não substituem o modelo. Respostas
preservam o assunto da conversa. Os valores são resolvidos antes de salvar a proposta:
edições posteriores do modelo não mudam o rascunho aguardando aprovação. Usar modelo
em envio também aplica a faceta **Ler**; bloqueá-la bloqueia o uso do modelo.
`list_templates` usa **Listar**, `render_template`/`list_followups` usam **Ler**;
`set_followup`/`cancel_followup` usam **Gerenciar lembretes de resposta** (Perguntar
por padrão), com escopo real da conta da mensagem. O resultado identifica as regras.

### Aguardar resposta, sem envio automático

1. Abra uma mensagem realmente **enviada**. Vá a **Configurações → Geral → Lembretes
   de resposta**, clique **Usar mensagem selecionada** (ou informe o ID), escolha
   1–365 dias e opcionalmente um modelo. Clique **Aguardar resposta** e aprove a
   alteração no painel normal quando a política pedir.
2. O prazo conta períodos de 24 horas a partir da marcação. A cada 30 segundos,
   com o aplicativo aberto, uma resposta recebida mais recente na mesma conversa
   encerra o lembrete. Mensagens enviadas pelo próprio usuário não contam como resposta.
   Mudanças no identificador da conversa são acompanhadas. Estado/prazo e deduplicação
   ficam no SQLite do perfil; após reiniciar, pendências vencidas são recuperadas.
3. Ao vencer sem resposta, há aviso no aplicativo e notificação do sistema sem
   conteúdo do e-mail. A entrega ao sistema é no máximo uma tentativa por lembrete;
   se o sistema recusar, a lista local continua disponível. Não existe serviço de
   fundo quando o aplicativo está fechado, nem detecção de respostas ainda não sincronizadas.
4. **Preparar retorno educado** é um clique explícito que cria uma proposta baseada
   no modelo (sem chamada de IA), preserva Para/Cc, exclui os próprios endereços e não
   reutiliza Cco. **Nunca envia por conta própria**, mesmo com regra Responder=Permitir.
   A proposta sempre exige aprovação no aplicativo. Uma nova resposta ou cancelamento
   antes da aprovação invalida a proposta. Variáveis adicionais de um modelo personalizado
   devem ser preenchidas em Escrever/Modelos; não são inventadas pela IA.
5. Revise assunto, destinatários e corpo no painel e aprove/cancele normalmente.
   Não há nova tentativa automática após falha ambígua. Propostas em memória expiram
   com o processo/prazo normal; o rascunho continua no perfil. Para propor novamente
   após cancelar/expirar/reiniciar, marque explicitamente outro prazo no lembrete
   (evita propostas duplicadas). A lista exibe até 200 lembretes ordenados por prazo.

**Windows:** na branch `feat/ai-mcp`, execute `cargo build -p flectar-mail -p mail-mcp`;
abra `target\debug\flectar-mail.exe` e conecte seu cliente ao
`target\debug\mail-mcp.exe` no mesmo perfil. O bridge, a aprovação local, chaves OAuth
armazenadas e opções de anexos permanecem iguais às seções anteriores. Esta fase foi
validada em Linux com perfil sintético, sem contas/e-mails/uploads reais; a notificação
Windows depende das permissões locais. Captura real da UI com dados sintéticos:
[`images/followups-pt-BR.png`](images/followups-pt-BR.png).

O resumo diário continua por datas locais; a janela móvel opcional de últimas 24 h
não foi incluída nesta fase. IA para elaborar retornos também não é necessária nem
utilizada: os modelos locais funcionam sem chave.

## Reply templates and follow-up reminders (English)

**Compose → Templates** reuses the existing local create/edit/delete interface. Five
clearly editable Portuguese examples are seeded once (quote, duplicate boleto,
payment confirmation, thank-you, polite follow-up). Deleted/edited seeds stay that way.
Use `{{nome}}`, `{{valor}}`, `{{data}}`, `{{vencimento}}`, `{{empresa}}`,
`{{assinatura}}` or custom Unicode/alphanumeric/underscore/hyphen/dot keys. Enter one
`key=value` per line; values may contain `=`. MCP string values may be multiline.
Existing per-account new/reply signature defaults fill `assinatura` unless explicitly
overridden; no fictional business identities are created. Managed compose signatures
are not inserted twice; manually edited signatures are retained for review.

Missing names are shown in red and prevent insertion/sending. Incomplete drafts may
be saved, but the central send queue also rejects unresolved variables in subject,
text or HTML. This conservative check includes literal mustache placeholders in quoted
mail. Variables are plain data, never executable expressions, HTML or recursive macros;
rich compose escapes inserted text. Stored limits: 200-byte name, 4-KiB subject,
64-KiB body; render limits: 100 variables, 16 KiB each, 128-KiB output.

MCP examples above are illustrative only (`example.invalid`):
- `list_templates`: up to 200 metadata records, **List** policy; bodies are not returned.
- `render_template`: `subject`, plain `body`, `missing`; **Read** policy; no mail sent.
- `send_message`/`reply_message` accept `template_id` and string-map `variables`.
  Without a template, the original required subject/body rules apply. Template content
  takes precedence over separate subject/body fields; replies retain the thread subject.
  Rendering happens before staging, and later template edits do not alter that draft.
  Template-based sends also evaluate the **Read** facet.
- `list_followups`: **Read**; `set_followup(messageId, days=3, template_id?)` and
  `cancel_followup(messageId)`: **Manage follow-up reminders**, **Ask** by default.
  All use the central evaluator/account scope and body/credential-free history; tool
  responses identify applicable rules. Standard app-only review defaults still apply.

Open **Settings → General → Follow-up reminders**, use the selected truly sent
message (or its ID), set 1–365 elapsed 24-hour days, optionally select a template ID,
and approve the tracking change if requested. Drafts/unsent outbox messages are rejected.
An incoming later message in the current thread completes the reminder; outgoing mail
does not. Tracking survives rethreading and is stored in the profile SQLite database.
The open app checks every 30 seconds and catches up overdue reminders after restart.
It shows an in-app notice and a generic OS notification without email content, with
at-most-once OS delivery attempts. There is no closed-app background service; replies
must have synced locally. Up to 200 reminders are shown, oldest due first.

**Prepare polite follow-up** is an explicit user click, not a scheduler action. It
uses a deterministic local template, retains original To/Cc minus own addresses, omits
Bcc, and creates an immutable proposal for app approval. It never sends automatically,
even under Reply=Allow; app-only approval is mandatory. A new reply or cancelled
reminder invalidates a staged proposal. Fill additional custom variables manually in
Compose/Templates. No AI key is needed or queried. Cancel/expiry/restart does not
automatically retry or duplicate proposals: explicitly set the reminder again to
propose another draft; drafts survive while in-memory approvals do not.

Windows try-it: `cargo build -p flectar-mail -p mail-mcp`, then open
`target\debug\flectar-mail.exe` and connect `target\debug\mail-mcp.exe` to the same
profile using the existing bridge instructions. This phase is Linux-debug validated
with synthetic profiles only, no real account/email/upload; native Windows notification
permissions remain platform-dependent. [Synthetic Slint capture](images/followups-pt-BR.png).
The optional rolling-last-24-hours digest window was deferred; existing local-date
ranges are unchanged.
