# CrabCraft: funcionalidades e status

Lista de tudo o que o **RustRover 2026.2** oferece, com a situação de cada item no CrabCraft.
É o documento de acompanhamento do projeto: **atualize o status no mesmo commit que implementa
a funcionalidade**.

## Legenda

| Status | Significado |
| --- | --- |
| ✅ Pronto | Funciona com projetos Rust e foi verificado no CrabCraft (teste ou uso real). |
| 🚧 Em andamento | Implementação parcial; a coluna "Plano" diz o que falta. |
| 🟢 Herdado | Veio do DotCraft e não depende de linguagem. Ainda não foi verificado em Rust. |
| 🔁 Portar | Existe no DotCraft para .NET e precisa ser refeito para Rust/Cargo. |
| ⬜ A fazer | Não existe ainda. |
| 🚫 Fora do escopo | Decidimos não fazer por enquanto; o motivo está no plano. |

Regras de honestidade: "Herdado" não é "Pronto", e paridade com o RustRover nunca é presumida.
Um item só vira ✅ com evidência de que funciona num projeto Rust real.

## Resumo

<!-- Atualize com scripts/features_summary.py -->
<!-- summary:start -->
| Status | Itens |
| --- | --- |
| ✅ Pronto | 163 |
| 🚧 Em andamento | 8 |
| 🟢 Herdado | 22 |
| 🔁 Portar | 1 |
| ⬜ A fazer | 59 |
| 🚫 Fora do escopo | 5 |
| **Total** | **258** |
<!-- summary:end -->

## Marcos

| Marco | Entrega |
| --- | --- |
| **M0 Base** | Marca CrabCraft, repositório, este inventário, build de referência. |
| **M1 Núcleo Rust** | Abrir workspace Cargo, realce Rust/TOML, rust-analyzer, build, run e diagnósticos. |
| **M2 Testes e depuração** | `cargo test` com árvore de resultados, depurador GDB/LLDB via DAP. |
| **M3 Produtividade** | Clippy/rustfmt integrados, janela Cargo, Run Anything, dependências, refatorações. |
| **M4 Exclusivos Rust** | Macros, features/cfg, lifetimes, Cargo.toml inteligente, REPL, cobertura. |
| **M5 Equipe e web** | Git avançado, GitHub/GitLab, HTTP Client, Docker, Axum. |
| **M6 Avançado** | Banco de dados, remoto, Dev Containers, plugins, IA. |

---

## 1. Toolchain e projetos Rust

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| P-01 | Detectar toolchain (rustc, cargo, stdlib) e versão | ✅ | M1 | Configurações > Ferramentas mostra versões reais e a stdlib, ou como instalá-la; vista na janela nativa |
| P-02 | Instalar rustup e stdlib pela IDE | 🚧 | M3 | Feito: com rustup, botões Instalar para `rust-src` e `rust-analyzer` em Configurações › Ferramentas (`rustup component add` na janela Run, testado com rustup real). Falta: instalar o próprio rustup pela IDE, com confirmação |
| P-03 | Avisar e aplicar atualização da toolchain | ✅ | M3 | Configurações › Ferramentas › Atualizações: Procurar atualizações roda `rustup check` em segundo plano só quando pedido (usa a rede) e lista cada toolchain e o rustup; Atualizar roda `rustup update` na janela Run (Rerun e Stop) e as versões são relidas ao terminar. Testado com rustup real (1.29.2 → 1.29.3 listado); captura conferida |
| P-04 | Toolchain WSL | ⬜ | M6 | |
| P-05 | Toolchain Ferrocene (`criticalup.toml`) | ⬜ | M6 | |
| P-06 | Variáveis de ambiente do projeto | ✅ | M3 | Por projeto (sessão), em Arquivo › Variáveis de ambiente do projeto…; aplicadas a todo comando cargo (metadata, build, run, test, janela Cargo), aos programas e ao rust-analyzer (reinicia ao mudar); as da configuração de execução prevalecem; testado com cargo e rust-analyzer reais |
| P-07 | Novo projeto: Binary e Library | ✅ | M1 | `cargo new` com Binary e Library; criação testada com Cargo real |
| P-08 | Novo projeto: WebAssembly e Procedural Macro | 🚧 | M4 | Feito: modelos Procedural Macro (`proc-macro = true`, syn/quote/proc-macro2, derive de exemplo e teste em `tests/`) e WebAssembly (`cdylib` + `rlib`, wasm-bindgen, função `#[wasm_bindgen]`) no diálogo Novo projeto; o diálogo avisa se faltam o target `wasm32-unknown-unknown` ou o wasm-pack, sem instalar nada. Testado com Cargo real: a macro passa em `cargo check` e `cargo test`; a biblioteca WebAssembly passa em `cargo test` nativo. Falta: verificar `cargo check --target wasm32-unknown-unknown` com o target instalado |
| P-09 | Modelos cargo-generate (repositório Git) | ⬜ | M4 | |
| P-10 | Configuração de run criada junto com o projeto | ✅ | M1 | Projeto novo abre com a config já selecionada e roda; teste real |
| P-11 | Janela Project (árvore de arquivos e crates) | ✅ | M1 | Árvore de arquivos e Bibliotecas externas (stdlib e crates do `Cargo.lock`, somente leitura); visto na janela nativa com `serde`. Ícones de `.rs`, `.toml`, `Cargo.toml`, `Cargo.lock` e `.lock`; ao lado de um `Cargo.toml`, `src`/`examples` com ícone de fontes, `tests`/`benches` de testes e `target` de excluída; captura offscreen clara e escura conferida |
| P-12 | Dependências externas na árvore do projeto | ✅ | M3 | Nó Bibliotecas externas, ver P-11 |
| P-13 | Novo arquivo Rust com anexar ao módulo (`mod x;`) | ✅ | M3 | Menu Adicionar em projetos Cargo; escreve o `mod` no pai, no editor se estiver aberto; visto na janela nativa |
| P-14 | Novo módulo (arquivo/diretório, público/privado) | ✅ | M3 | Arquivo Rust ou Módulo Rust (`x/mod.rs`), com opção `pub mod`; `#[path]` não é considerado |
| P-15 | Novo crate Cargo no workspace | ✅ | M3 | Adicionar > Crate Cargo roda `cargo new` e recarrega o projeto; sem `[workspace]` o diálogo avisa que o crate fica de fora; visto na janela nativa |
| P-16 | Aviso de arquivo fora de módulo, com anexar | ✅ | M3 | Detectado por `experimental/parentModule`; faixa no editor com botão Declarar; visto na janela nativa |
| P-17 | Janela Cargo: membros e targets | ✅ | M3 | Lê `cargo metadata`, com recarregar; vista na janela nativa |
| P-18 | Executar target com duplo clique | ✅ | M3 | bin na janela Run; lib, example, test e bench no console de Compilar; `lib` clicado na janela nativa, demais por teste real |
| P-19 | Executar comando Cargo pela janela Cargo | ✅ | M3 | Campo com Enter; `cargo` no início é opcional; falha mostra o código de saída |
| P-20 | Anexar/desanexar projetos Cargo | ✅ | M3 | No explorador, um `Cargo.toml` que não é o do workspace tem Anexar/Desanexar projeto Cargo: o rust-analyzer passa a analisá-lo (`linkedProjects`, com o do workspace) e reinicia; seus binários viram configurações `bin (pasta)` que compilam com o próprio manifesto; a janela Cargo lista seus pacotes como anexados; a carga aparece na aba Sync; guardado na sessão. Testado com rust-analyzer e Cargo reais num crate em `tools/gen`; captura conferida |
| P-21 | Recarregar modelo ao mudar `Cargo.toml` (automático) | ✅ | M1 | Editar Cargo.toml recarrega os alvos, testado com cargo real |
| P-22 | Botão Load Cargo Changes (manual) | ✅ | M3 | Botão recarregar na janela Cargo; alterar `Cargo.toml` também recarrega sozinho |
| P-23 | Aba Sync com o log do carregamento | ✅ | M3 | Aba Sync na janela Build (projetos Cargo): cada `cargo metadata` do modelo e das dependências, com duração, hora e o que o Cargo imprimiu (o comando e o erro, quando falha); a aba diz "falhou" quando o modelo não carregou. Testado com Cargo real e um `Cargo.toml` quebrado; captura clara e escura conferida |
| P-24 | Excluir pastas e arquivos da análise | ✅ | M3 | Menu de contexto do explorador (Excluir da análise / Incluir novamente), salvo por projeto na sessão; pastas e arquivos excluídos ficam fora do índice (Ir para arquivo, Search Everywhere), do Find in Files e dos eventos enviados ao servidor, aparecem esmaecidos no explorador e vão para o rust-analyzer em files.exclude (reinicia o servidor ao mudar). target/ já era oculto. |
| P-25 | Invalidar caches | ✅ | M3 | Arquivo > Invalidar caches: relê os arquivos abertos, refaz o índice de arquivos, o status do Git, o modelo do Cargo e o rust-analyzer sem fechar abas; cargo clean opcional. Testado com Cargo e rust-analyzer reais |
| P-26 | Segurança: projeto confiável antes de rodar build scripts | ✅ | M1 | rust-analyzer só inicia após confiar; teste prova que `build.rs` não roda antes |
| P-27 | Estilo de código compartilhado no projeto | ✅ | M3 | `.editorconfig` (seções com globs, `root`, indentação, `tab_width`, `max_line_length`, `trim_trailing_whitespace`, `insert_final_newline`, `end_of_line` em arquivo novo) e, para `.rs`, `rustfmt.toml`/`.rustfmt.toml` (`hard_tabs`, `tab_spaces`, `max_width`, `newline_style`), lidos só dentro do projeto; o rustfmt prevalece em Rust. Indentação e margem direita ao abrir, limpeza ao salvar (menos na linha do cursor); a barra de status diz de onde veio. Visto na janela nativa com `hard_tabs` e `max_width = 60` |
| P-28 | Clonar projeto de Git na tela inicial | 🟢 | M1 | Confirmar a cobertura herdada |
| P-29 | Abrir projeto por `Cargo.toml` ou pasta | ✅ | M1 | Abrir pasta ou Cargo.toml testado; dentro do projeto o Cargo.toml abre para edição |

## 2. Editor

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| E-01 | Editor rápido com arquivos grandes | 🟢 | M1 | |
| E-02 | Realce de sintaxe Rust | ✅ | M1 | Consultas próprias sobre tree-sitter-rust; testado e visto no app |
| E-03 | Realce de TOML | ✅ | M1 | Consultas próprias sobre tree-sitter-toml-ng; testado no parser real |
| E-04 | Realce semântico (tipos inferidos, macros, `unsafe`) | ✅ | M1 | Tokens semânticos do rust-analyzer; testado |
| E-05 | Dobramento de código | ✅ | M1 | Itens, `match`, macros e comentários; testado no parser real |
| E-06 | Múltiplos cursores e seleção | 🟢 | M1 | |
| E-07 | Colchetes pareados e fechamento automático | 🟢 | M1 | Conferir `'` em lifetimes e `|` em closures |
| E-08 | Comentar linha/bloco | ✅ | M1 | `//` e `/* */` em Rust, `#` em TOML; testado |
| E-09 | Sticky lines | ✅ | M3 | Até 5 linhas de abertura dos blocos em volta do topo (dobras do tree-sitter) ficam fixas; clique leva à linha; opção em Configurações > Editor. Teste de UI em código Rust e captura conferida |
| E-10 | Zoom do editor e da IDE | 🟢 | M1 | |
| E-11 | Temas claro/escuro e esquemas de cores | 🟢 | M1 | |
| E-12 | Salvar e reverter alterações | 🟢 | M1 | |
| E-13 | Local History | ✅ | M3 | Cada salvamento e recarga guarda a versão anterior e a nova em history/ na pasta de dados (rc-core::history: hash do caminho, sem duplicatas consecutivas, até 100 versões e 30 dias), gravadas numa thread própria. "Show Local History" (menu da aba, Find Action, menu File) lista as versões com prévia das linhas alteradas, "Show Diff" lado a lado e "Revert to This Version" como edição desfazível. Teste de UI num projeto Cargo: screens::local_history_lists_saved_versions_and_reverts_as_an_undoable_edit. |
| E-14 | LightEdit (arquivo sem projeto) | ✅ | M3 | Arquivo aberto sem projeto (CLI, Abrir…, arrastar) fica em modo LightEdit: só edição, sem rust-analyzer, com "LightEdit" na barra de status; se ele pertence a um projeto Cargo (o `[workspace]` mais externo ou o `Cargo.toml` mais próximo), uma faixa oferece Abrir projeto. Teste headless e captura clara e escura conferida |
| E-15 | Corretor ortográfico e gramática | ⬜ | M6 | |
| E-16 | Live templates e variáveis | ✅ | M3 | Configurações › Live Templates: abreviação, descrição e corpo com `$1`, `${2:padrão}` e `$0`, com modelos iniciais (`p`, `pd`, `main`, `fn`, `test`, `tmod`, `st`, `en`, `impl`, `ifl`, `matchr`) e Restaurar padrões. Em arquivo Rust, abreviação + Tab expande na indentação da linha; Tab e Shift+Tab percorrem as paradas, Esc encerra. Snippets da completação do rust-analyzer ganharam as mesmas paradas. Teste headless e captura conferida. Falta: variáveis com expressões e contextos além de Rust |
| E-17 | Postfix completion (`.if`, `.match`…) | ✅ | M3 | Postfix chega ao completar, testado com rust-analyzer real |
| E-18 | Pré-visualização de Markdown | 🟢 | M1 | |
| E-19 | Suporte a JSON e XML | 🟢 | M1 | |
| E-20 | Suporte a HTML, CSS, JavaScript e TypeScript | ⬜ | M6 | Realce primeiro |
| E-21 | Emmet | 🚫 | - | Depende do suporte web |
| E-22 | Cabeçalhos de copyright | ✅ | M6 | Configurações › Copyright: perfil do projeto salvo na sessão, com modelo (`$year`, `$file`, `$project`, `$author` = `user.name` do Git, lido numa thread), licença SPDX opcional (expressão inválida fica de fora), opção de pôr o cabeçalho em arquivos novos e prévia. Code › Update Copyright (menu, Find Action) insere o cabeçalho no estilo de comentário da linguagem (`//` em Rust, `#` em TOML, bloco onde não há comentário de linha; JSON não recebe) depois de shebang/BOM/declaração XML e antes de `#![…]`, ou só estende o ano do cabeçalho existente (`2020` → `2020-2026`, também em `/* */`), como edição desfazível; doc comments não contam como cabeçalho. Modelo vindo da sessão é limitado e o casamento não degenera. Teste headless num projeto Cargo com Git real (`screens::copyright_headers_on_new_and_old_files`: New › Rust File, atualização idempotente) e num `main.rs` real do CrabCraft. Captura da página não conferida. Falta: vários perfis por escopo de arquivos e atualizar o projeto inteiro de uma vez |
| E-23 | Breadcrumbs | ✅ | M3 | Barra de status mostra pasta, arquivo e os símbolos em volta do cursor; visto na janela nativa; o clique no símbolo não foi exercitado lá |
| E-24 | Marcas de erro na barra de rolagem | 🟢 | M3 | Herdado; marca de erro de Rust vista na janela nativa |
| E-25 | Lens mode na barra de rolagem | ✅ | M6 | Passar o ponteiro na barra de rolagem mostra 9 linhas do código naquela altura (cores, números, problemas sublinhados, dobras respeitadas); sobre a marca de um erro mostra a mensagem. Desligável em Settings › Editor. Teste headless em `crates/rc-editor/src/view.rs` deste repositório (`the_scrollbar_lens_previews_the_code_under_the_pointer`) e captura GPU `scrollbar_lens_capture`. |
| E-26 | Abas: fixar, dividir, destacar, pré-visualização | ✅ | M1 | Grupos de abas divididos em qualquer direção; arrastar reordena, move entre grupos, divide pela borda ou solta fora da janela para flutuar; janelas flutuantes com várias abas; layout salvo na sessão. Teste de UI num projeto Cargo arrasta para dividir e para fora; arrasto entre janelas reais do sistema não exercitado |
| E-27 | Scratch files | ✅ | M3 | Novo arquivo de rascunho (Ctrl+Alt+Shift+Insert / ⌘⇧N): Rust, TOML, Markdown, JSON ou texto, guardado fora do projeto e listado para reabrir. Com um projeto aberto, rascunho Rust é analisado sozinho pelo rust-analyzer (arquivo avulso em `linkedProjects`); o botão Run da faixa compila com `rustc` numa pasta temporária e roda na janela Run (Rerun recompila; erro do rustc impede a execução). Testado com rust-analyzer e rustc reais |

## 3. Inteligência de código

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| I-01 | Análise do projeto (índice) | ✅ | M1 | rust-analyzer com estado de análise na barra de status; testado e visto no app |
| I-02 | Autocompletar com documentação | ✅ | M1 | Testado com rust-analyzer real |
| I-03 | Completar em `#[derive]`, macros e doctests | 🚧 | M1 | Derive e macro testados com rust-analyzer real; em doctest ele não trouxe sugestões na sonda, exige solução própria |
| I-04 | Auto-import ao completar | ✅ | M1 | Import ao aceitar o item testado com rust-analyzer real |
| I-05 | Auto-import ao digitar e ao colar | ✅ | M3 | Ao digitar: aceitar uma sugestão da completação de um item de outro módulo adiciona o `use` (testado). Ao colar código Rust: para cada nome tipo-tipo do trecho, o rust-analyzer é consultado; os que têm uma só importação ganham a linha `use` (após os `use` do arquivo, ou após comentários e atributos iniciais) e os ambíguos são avisados para escolher em Mostrar ações de contexto; opção em Configurações › Editor. Testado com rust-analyzer real (`HashMap` importado, `Ordering` ambíguo). Falta: importar sem completação enquanto digita |
| I-06 | Excluir itens do auto-import | ✅ | M3 | Configurações › Editor › Nunca importar: caminhos (item ou módulo, um por linha) que a completação nunca oferece importar, via `completion.autoimport.exclude` do rust-analyzer, aplicados sem reiniciar o servidor. Testado com rust-analyzer real: `std::sync::atomic::Ordering` some das sugestões e `std::cmp::Ordering` fica |
| I-07 | Adicionar crate popular ao colar código | ⬜ | M4 | |
| I-08 | Sugerir crates ainda não adicionados | ⬜ | M4 | |
| I-09 | Ordenação de completar por ML | 🚫 | - | Sem modelo próprio; rust-analyzer ordena |
| I-10 | Hover e documentação rápida | ✅ | M1 | Hover testado com rust-analyzer real |
| I-11 | Abrir documentação em docs.rs | ✅ | M3 | Shift+F1 pede a página a `experimental/externalDocs` e abre no navegador; endereço testado com rust-analyzer real, abertura do navegador não exercitada |
| I-12 | Informação de parâmetros | ✅ | M1 | Assinatura testada com rust-analyzer real |
| I-13 | Inlay hints (tipos, parâmetros, chaining) | ✅ | M1 | Tipos e parâmetros testados com rust-analyzer real |
| I-14 | Diagnósticos em tempo real | ✅ | M1 | Testado com rust-analyzer real: erro de sintaxe ao digitar e erro do cargo check ao salvar |
| I-15 | Quick fixes e intention actions (Alt+Enter) | ✅ | M1 | Import pelo Alt+Enter testado com rust-analyzer real |
| I-16 | Aplicar correção sugerida pelo compilador | ✅ | M3 | Correção do rustc aparece no Alt+Enter; "remove this `mut`" aplicada em teste real |
| I-17 | Inspeções em todo o projeto | ✅ | M3 | Inspecionar código (Find Action, menu nativo ou a aba Inspeção da janela Problems) roda `cargo clippy --workspace --all-targets` em segundo plano, cancelável pela janela Build; os achados, também de arquivos nunca abertos, ficam na aba Inspeção agrupados por lint, com clique para abrir na linha. Testado com Clippy real num workspace de dois crates; captura conferida |
| I-18 | Perfis e severidade de inspeções | ⬜ | M4 | |
| I-19 | Suprimir inspeções (`#[allow]`) | ✅ | M3 | Alt+Enter num aviso do rustc ou do Clippy oferece suprimir o lint na instrução ou item (junta no #[allow] existente) ou no arquivo (#![allow]); só para nomes de lint, não para checagens próprias do rust-analyzer. Testado com rust-analyzer e cargo check reais |
| I-20 | Linters externos: Cargo Check e Clippy | ✅ | M3 | `check.command` do rust-analyzer, trocado sem reiniciar; lint `len_zero` do Clippy recebido em teste real |
| I-21 | Widget do linter na barra de status | ✅ | M3 | Clique alterna Cargo Check/Clippy e salva a escolha; visto na janela nativa |
| I-22 | Inspeções de `Cargo.toml` | ✅ | M4 | Offline, numa thread, a cada mudança do `Cargo.toml` aberto: chave desconhecida (com sugestão), dev-dependency com requisito incompatível com o de `[dependencies]`, dependência sem origem, feature que cita o que não existe ou dependência não opcional, `dep:` sem feature implícita, edição inválida, membro do workspace ou dependência `path` sem pacote. Diagnósticos no editor e na janela Problemas; Alt+Enter corrige (trocar chave/valor, tornar opcional). Testado na interface com um projeto real. |
| I-23 | Visualizar lifetimes em erros do borrow checker | ⬜ | M4 | A partir dos spans do rustc |
| I-24 | Explicações inline de erros | ✅ | M4 | Para diagnósticos com código do rustc (`E` e quatro dígitos, validado antes de chamar o rustc), "Explicar E0308" no hover do erro no editor e no menu de contexto das janelas Problemas e Build abre a explicação de `rustc --explain` (o rustc ao lado do cargo, em segundo plano com tempo limite e cache por código) numa aba Markdown somente leitura. Sem explicação, avisa. Testado em projeto Cargo real: E0308 de `cargo check` ao salvar e do build, com rustc e rust-analyzer reais; visto na janela renderizada |
| I-25 | Expandir macro (recursivo e um passo) | 🚧 | M4 | Expandir recursivamente (menu Código e Find Action) abre a expansão numa aba somente leitura, fora do rust-analyzer; testado com rust-analyzer real. Falta expandir um passo: o rust-analyzer só expande tudo |
| I-26 | Testador interativo de macros declarativas (2026.2) | ⬜ | M4 | |
| I-27 | Reformatar com rustfmt | ✅ | M3 | Reformat Code via rust-analyzer testado |
| I-28 | rustfmt ao salvar e ao commitar | ✅ | M3 | Ao salvar (Configurações > Editor) e ao commitar (janela Commit); só os arquivos incluídos, e um erro de sintaxe cancela o commit sem alterar nada |
| I-29 | Configurar rustfmt (argumentos, canal, env) | ✅ | M3 | Configurações > Ferramentas: argumentos, canal (via rustup, +canal) e variáveis de ambiente; valem para Reformat Code, ao salvar e ao commitar (o mesmo comando rustfmt, sem depender do rust-analyzer); erros de configuração aparecem no diálogo e cancelam o commit |
| I-30 | Formatador embutido para trechos | ✅ | M3 | Reformatar código com seleção num arquivo Rust: o rustfmt (com as configurações do usuário) formata o arquivo em segundo plano e só os blocos de mudança que tocam as linhas selecionadas são aplicados (um bloco que cruza a borda vai inteiro). Testado com rustfmt real: só a função selecionada mudou |
| I-31 | Completar linha inteira local | ⬜ | M6 | Pode usar ollama |
| I-32 | Otimizar imports | ✅ | M3 | Ctrl+Alt+O remove os não usados e junta o resto, com assists do rust-analyzer; ordenar fica com o rustfmt; visto na janela nativa |
| I-33 | Pré-visualizar intention antes de aplicar | ⬜ | M4 | |

## 4. Navegação e busca

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| N-01 | Search Everywhere (Shift duplo) | ✅ | M1 | Ctrl+P/⌘P e Shift duplo; abas Todos, Classes, Arquivos, Símbolos, Ações e Texto (busca em texto em segundo plano, cancelável); escopo Workspace, Workspace e dependências ou um crate; testado com kittest e rust-analyzer real |
| N-02 | Ir para arquivo, tipo e símbolo | ✅ | M1 | workspace/symbol testado com rust-analyzer real |
| N-03 | Ir para definição | ✅ | M1 | Ctrl+clique (⌘ no macOS) e Ctrl+B levam à definição: método, tipo e item da stdlib (aberto somente leitura), testado com cliques reais e rust-analyzer real |
| N-04 | Ir para implementações | ✅ | M1 | Implementações de trait testadas com rust-analyzer real |
| N-05 | Encontrar usos | ✅ | M1 | Usos testados com rust-analyzer real |
| N-06 | Find in Files e Replace in Files | 🟢 | M1 | |
| N-07 | Arquivos e locais recentes | 🟢 | M1 | |
| N-08 | Estrutura do arquivo | ✅ | M1 | documentSymbol testado com rust-analyzer real |
| N-09 | Hierarquia de chamadas | ✅ | M3 | Janela Hierarquia com árvore: Ctrl+Alt+H abre quem chama, botão troca para o que é chamado, cada nível carrega ao abrir; visto na janela nativa |
| N-10 | Hierarquia de tipos e traits | 🚧 | M3 | Ctrl+Alt+Shift+H lista os `impl` de um trait ou tipo na janela Hierarquia, via `textDocument/implementation`, um nível; supertraits e subtraits faltam, o rust-analyzer não tem hierarquia de tipos |
| N-11 | Ir para o trait / para o `impl` | ✅ | M3 | Ctrl+U vai do método do `impl` ao trait; Ctrl+Alt+B vai do trait aos `impl`; teste real |
| N-12 | Bookmarks | ✅ | M3 | Ctrl+F11 marca a linha, Ctrl+Shift+F11 lista; acompanham edições e sobrevivem ao fechar a aba; sem atalho F11 porque ele é Step Into |
| N-13 | Navegar entre features e `cfg` | ✅ | M4 | Sem servidor: no `Cargo.toml`, Ctrl+B, Ctrl+clique ou Localizar usos na chave de `[features]` lista na janela Localizar os `cfg`, `cfg!` e `cfg_attr` (inclusive dentro de `all`/`any`/`not`) do código do pacote, em segundo plano e cancelável, com o texto não salvo das abas; numa feature ativada por outra (`full = ["std"]`) Ctrl+B vai à linha dela e Voltar retorna. Num `.rs`, Ctrl+B ou Ctrl+clique em `"x"` de `feature = "x"` leva à linha da feature no `Cargo.toml` do pacote. Ignora comentários, strings comuns, `target/`, pastas ocultas e pacotes aninhados; `dep:x` e `x/y` não são features. Testado de ponta a ponta (headless) num crate de exemplo e a análise no crate real egui 0.36.2 do registro (serde, persistence, callstack); captura da janela não conferida |
| N-14 | Navegar para a fonte de dependências | ✅ | M3 | Ctrl+B entra na stdlib e abre somente leitura; teste real, crates do registro pelo mesmo caminho |
| N-15 | Scopes e cores de arquivo | ✅ | M4 | Escopos embutidos (Arquivos do projeto, Testes, Arquivos alterados pelo git) e do projeto, por globs com `!` para excluir, salvos na sessão (Configurações › Escopos). Find in Files filtra por escopo; cores de arquivo pintam o Explorer e as abas (vale o primeiro escopo que contém o arquivo; ordem ajustável). Padrão inválido mostra o erro. Testado num projeto git e no próprio repositório do CrabCraft; capturas conferidas. |
| N-16 | Navigation bar e Select In | ✅ | M3 | Barra clicável na parte de baixo (pastas, arquivo e símbolos). Select In (Alt+F1) leva ao Explorador ou ao gerenciador de arquivos; sem outros destinos, como a janela Estrutura. |
| N-17 | Mostrar contexto do código (Alt+Q) | ✅ | M4 | Navegar › Mostrar informações de contexto (Alt+Q no keymap Rider; ⌃⇧Q no macOS; sem atalho nos keymaps VS e VS Code; também em Localizar ação): popup acima do cursor com os cabeçalhos das declarações que o envolvem e estão fora da vista (mod, impl, trait, fn, struct, enum, macro_rules!, closures), do mais externo ao mais interno, com número da linha e cores da fonte do editor; assinaturas de várias linhas viram uma (ou primeira linha + …). Clique vai à linha; Esc, digitar ou mover o cursor fecha. Outras linguagens usam os blocos de dobra. Testado em src/app.rs deste repositório (teste headless e captura conferida) |
| N-18 | Compartilhar no Rust Playground | 🚧 | M4 | Feito: "Compartilhar no Playground…" no menu de contexto do editor (arquivos `.rs`), no menu Code e em Find Action. Envia a seleção, ou o arquivo inteiro, só depois de uma confirmação que avisa que o código vira um gist público do GitHub via play.rust-lang.org, diz quantas linhas vão e mostra as primeiras; nela se escolhe o canal (stable/beta/nightly) e a edição (padrão: a do crate). O POST `{"code": …}` para `/meta/gist` roda em segundo plano, com timeout e limite de tamanho da resposta; o id da resposta é validado, o link `?version=…&mode=debug&edition=…&gist=…` é copiado e aparece na barra de status com "Abrir no navegador"; erros viram mensagens. Testado com um servidor HTTP local no lugar do Playground (cancelar não envia nada; confirmar envia o JSON com o User-Agent e copia o link). Falta: o envio real a play.rust-lang.org não foi exercitado (publicaria um gist); o teste `live_upload_to_the_playground` fica `#[ignore]` |

## 5. Refatoração e geração de código

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| R-01 | Lista de refatorações (Ctrl+Alt+Shift+T) | ✅ | M3 | Popup com Renomear e as refatorações do rust-analyzer (tipo `refactor*`); também no menu de contexto e no menu Refatorar. |
| R-02 | Renomear em todo o projeto | ✅ | M1 | Rename entre arquivos testado com rust-analyzer real |
| R-03 | Pré-visualização de refatoração | ✅ | M3 | Renomear símbolo › Pré-visualizar lista as edições do rust-analyzer por arquivo, cada linha antes → depois com caixa de seleção (por arquivo e por mudança); Refatorar aplica só as marcadas (arquivos abertos como um passo de desfazer, fechados no disco). Renomear arquivo já mostrava a lista. Testado com rust-analyzer real; captura conferida. Falta: prévia das demais refatorações (ações de código) |
| R-04 | Lista de conflitos da refatoração | ✅ | M3 | Antes de aplicar (ou pré-visualizar) um Renomear em Rust, as edições do rust-analyzer são verificadas numa thread: nome já usado num arquivo tocado (fora de comentários e strings), palavra-chave ou identificador inválido, arquivo fora do projeto, em `target/`, em dependência/toolchain (`.cargo/registry`, `.rustup`), somente leitura ou com texto que não bate mais com a edição, e erro do rust-analyzer. A caixa lista cada conflito (arquivo:linha clicável que abre o local) com Continuar mesmo assim / Cancelar; sem conflitos o rename segue direto. Testado com rust-analyzer real; captura conferida. Falta: conflitos das demais refatorações (ações de código) e checagem de escopo exata (o choque de nome é por arquivo) |
| R-05 | Extrair variável, constante, função | ✅ | M3 | Extrair variável e função testados com rust-analyzer real |
| R-06 | Inline de variável e função | ✅ | M3 | Inline de variável testado com rust-analyzer real |
| R-07 | Mudar assinatura | ⬜ | M4 | |
| R-08 | Mover itens entre módulos | ⬜ | M4 | |
| R-09 | Popup Generate (Alt+Insert) | ✅ | M3 | Alt+Insert mostra só as ações que geram código; teste real, atalho não exercido na janela |
| R-10 | Implementar membros de trait | ✅ | M3 | Implementar membros de trait testado com rust-analyzer real |
| R-11 | Gerar `derive` | ✅ | M3 | Alt+Insert sobre struct, enum ou union mostra Derive…: marca os traits comuns e outros digitados; junta no #[derive] existente ou cria acima do item. Testado com rust-analyzer real e tree-sitter |
| R-12 | Renomear arquivo ou módulo com as referências | ✅ | M3 | F2 no Explorador (ou menu da aba): `workspace/willRenameFiles` do rust-analyzer (`mod`, caminhos `use`) e ocorrências do nome em texto, em segundo plano; lista com caixas por arquivo e linha, Renomear e atualizar / Só renomear. Teste real com rust-analyzer e `cargo check`; F2 testado com kittest. Pastas: só pelo servidor e por `nome/` em texto, sem teste real. |

## 6. Cargo.toml e conditional compilation

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| C-01 | Completar chaves do manifesto | ✅ | M4 | Local, sem rede nem rust-analyzer: cabeçalhos após `[`/`[[` (sem repetir tabelas que já existem), chaves da tabela atual (package, alvos, profile, workspace, lints, dependência em tabela ou inline) e valores fixos (edition, resolver, panic, lto, opt-level, strip, debug, booleanos). Abre ao digitar e com Ctrl+Espaço, com descrição traduzida no popup. Teste kittest digitando no Cargo.toml de um workspace Cargo real; análise em `rc_cargo::manifest_complete` com testes. Não completa nomes de crates, versões nem features dentro de listas |
| C-02 | Completar nome e versão de crates (crates.io) | ✅ | M4 | Numa tabela de dependências do `Cargo.toml`, digitar um nome abre a lista de crates: com a consulta online ligada, o texto digitado é buscado na API do crates.io (`/api/v1/crates`, o índice esparso não tem busca) depois de uma pausa de 300 ms, buscas obsoletas são canceladas e as respostas ficam em cache na sessão; cada item mostra a versão estável mais nova, os downloads e a descrição. Escolher um crate escreve `nome = ""` e abre as versões publicadas (índice esparso, sem yanked): a estável mais nova com as formas curtas (`1.0.229`, `1.0`, `1`) e as demais; também dentro de `version = ""` de `{ … }` e `[dependencies.x]`. Offline (consulta desligada ou sem rede), os nomes vêm do `Cargo.lock` e do cache local do Cargo (`~/.cargo/registry`) e as versões do lock; nada é enviado. Rede em thread de fundo, com timeout, limite de tamanho e resposta tratada como não confiável; User-Agent genérico `CrabCraft/<versão>`. Testado no app (teste headless com respostas fixas e servidor HTTP local) e com o crates.io de verdade (busca `egui`, versões de `serde`). Falta: registros alternativos e ordenar por relevância própria |
| C-03 | Mostrar versão do lock e a mais recente | ✅ | M4 | No `Cargo.toml` aberto, cada dependência (`[dependencies]`, dev, build, `target.*`, `[workspace.dependencies]`, `[dependencies.x]`, chaves pontuadas e `workspace = true`, que herda do manifesto do workspace) mostra no fim da linha a versão resolvida no `Cargo.lock` (com várias versões travadas, a mais nova que atende ao requisito) e, se houver, a mais recente do crates.io (`1.0.100 · 1.0.229 available`). O lock é lido localmente numa thread, com limite de tamanho; o crates.io é consultado pelo índice esparso (só o nome do crate vai na requisição), em segundo plano, com timeout e cache por sessão, ignorando versões yanked e pré-lançamentos (salvo se a atual for um). Dependências de path e git não mostram nada. Opção em Configurações › Editor para desligar a consulta online (ligada por padrão). Testado no app com um projeto gerado e lock real (teste headless) e no `crates/rc-cargo/Cargo.toml` do próprio CrabCraft com o crates.io de verdade. Falta: atualizar a versão pela dica (quick-fix) e consultar registros alternativos |
| C-04 | Avisar crates inexistentes ou desatualizados | ✅ | M4 | Com a consulta online ligada, cada dependência do crates.io no `Cargo.toml` aberto é conferida com o índice esparso: crate inexistente (404) vira erro no nome, requisito que nenhuma versão publicada (não yanked) atende vira aviso, e requisito que não aceita a estável mais nova vira aviso fraco com quick-fix (Alt+Enter: `Update to 1.0.229`, mantém `^`/`~`/`=`, desfazível). Erros e avisos também vão para a janela Problems; `workspace = true` é conferido no manifesto do workspace; nomes fora do lock só são consultados depois que o texto para por 800 ms. Desligar a consulta remove os diagnósticos; sem rede, nada é marcado. Testado no app (teste headless) e nos 14 `Cargo.toml` do próprio CrabCraft com o crates.io de verdade (59 crates, nenhum falso "inexistente"; avisa p.ex. `egui = "0.36"` com 0.37.0 publicado). Falta: registros alternativos, nova tentativa após falha de rede na mesma sessão e ignorar um aviso por dependência |
| C-05 | Gerenciador visual de dependências | 🔁 | M4 | Reescrever a janela NuGet com `cargo add`/`remove` |
| C-06 | `cargo tree` integrado | ✅ | M4 | Cada membro da janela Cargo tem um nó Dependências: ao abrir, roda `cargo tree --offline --locked` em segundo plano (não compila nem grava o `Cargo.lock`) e mostra a árvore dobrável, com versão, seção build/dev e repetições. A execução entra na aba Sync. Testado com o cargo real num projeto gerado e na árvore do próprio CrabCraft (1140 nós) |
| C-07 | Ligar/desligar features pela margem | ✅ | M4 | No `Cargo.toml` aberto, cada entrada de `[features]` tem um interruptor na margem; a janela Cargo tem um nó Features por membro com os mesmos interruptores. As escolhidas ficam na sessão do projeto (`pacote/feature`, validadas ao ler), vão para o rust-analyzer como `cargo.features` via `didChangeConfiguration` (sem reiniciar o servidor) e entram na atenuação de `cfg` (C-08). Features ligadas pelo `default` ou por outra feature aparecem ligadas e não se desligam pela margem; as que somem do manifesto são descartadas. Testado com o cargo e o rust-analyzer reais num projeto gerado: o build script vê `CARGO_FEATURE_EXTRA` ao ligar e deixa de ver ao desligar. Falta: passar `--features` em builds e configurações de execução, e `noDefaultFeatures` |
| C-08 | Atenuar código desativado por `cfg` | ✅ | M4 | O editor avalia cada `#[cfg(...)]` (itens, instruções, campos, variantes e braços de `match`) com os cfgs de `rustc --print cfg` e as features do `default` do pacote mais as ligadas pelo usuário (C-07), e atenua o que fica desligado. Não depende do rust-analyzer (o 1.99 não publica `inactive-code`) nem de confiar no projeto. Testado no app com um projeto gerado e no `instance.rs` do próprio CrabCraft. Falta: dica ao passar o mouse |
| C-09 | Seletor de target (triple) na barra de status | ✅ | M4 | A barra de status mostra o triple usado na análise (o `[build] target` do `.cargo/config.toml`, senão o host); o clique abre a lista com o host marcado, os targets do projeto, os instalados pelo rustup e os não instalados (`rustup target list`, rodado fora do projeto), com filtro quando a lista é longa. O triple é validado (`[A-Za-z0-9_.-]`) antes de ir a qualquer processo, fica na sessão do projeto, vai para o rust-analyzer como `cargo.target` via `didChangeConfiguration` (sem reiniciar) e recarrega `rustc --print cfg --target` para a atenuação (C-08). Testado com rustc, rustup e rust-analyzer reais num projeto gerado: ao escolher `x86_64-pc-windows-msvc` o `#[cfg(windows)]` deixa de ser atenuado e o `#[cfg(unix)]` passa a ser, e o build script do rust-analyzer vê `TARGET` mudar. Falta: passar `--target` em builds/checks iniciados pela IDE (escolha deliberada: mudaria a pasta dos binários usados por executar e depurar) |
| C-10 | Quick fix para habilitar feature | ✅ | M4 | Quando o `cargo check` (rodado pelo rust-analyzer ao salvar, só em projeto confiável) diz que um nome ou import está atrás de uma feature, Alt+Enter oferece ligá-la: feature de dependência entra no `features` dela no `Cargo.toml` (edição no editor, com desfazer, sem salvar); feature do próprio pacote liga na análise ou entra em `default`. Testado com tokio real (`fs`) e feature própria: os erros somem. |
| C-11 | Ícone da margem para abrir docs.rs do crate | ✅ | M4 | Cada dependência do crates.io no `Cargo.toml` aberto tem um ícone de link na margem que abre `https://docs.rs/<nome>/<versão do lock>` no navegador (`latest` se o crate não está no lock). Dependências de path, git e de outros registros não têm ícone; convive com os interruptores de features (C-07). Testado no app com um projeto gerado e no `crates/rc-cargo/Cargo.toml` do CrabCraft (abre `docs.rs/semver/1.0.28`). Falta: dica ao passar o mouse sobre o ícone |

## 7. Build e execução

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| B-01 | Build, rebuild e clean | ✅ | M1 | `cargo build` com JSON, rebuild, clean e fetch; build testado com Cargo real |
| B-02 | Janela Build com erros clicáveis | ✅ | M1 | Erro real de compilação clicado na janela nativa abriu o arquivo na linha e coluna |
| B-03 | Botão Apply fix em erros do compilador | ✅ | M3 | Na lista de Compilar; só aplica se o arquivo ainda for o que o compilador leu; não salva |
| B-04 | Janela Problems | ✅ | M1 | Vista na janela nativa com erro real, também de arquivo fechado; clique na lista só por código herdado, sem teste |
| B-05 | Configurações de run automáticas por target | ✅ | M1 | Uma por binário, mais All Tests; testado e visto no app |
| B-06 | Editor de configurações de run | ✅ | M3 | Diálogo visto na janela real |
| B-07 | Configuração Cargo: comando, canal, env, cwd, stdin | ✅ | M3 | Editar configuração: perfil, toolchain, features, ambiente, pasta e arquivo de entrada, testados no toolchain real, exceto a troca de toolchain (só os argumentos); outros comandos pela janela Cargo |
| B-08 | Features automáticas (`required-features`) | ✅ | M3 | `required-features` lidas do metadata e aplicadas sozinhas; teste real |
| B-09 | Emular terminal na saída | ✅ | M3 | Opção da configuração Cargo Emular terminal no console de saída: compila (e roda as tarefas antes de executar) e inicia o programa num pseudoterminal na aba Run, com cores, `isatty` verdadeiro e entrada pelo teclado; Stop e o código de saída funcionam. Testado com Cargo real (`is_terminal`, leitura de linha, saída 3). Falta: depurar com terminal emulado |
| B-10 | Executar como root | ⬜ | M6 | |
| B-11 | Múltiplas instâncias e configuração no projeto | ✅ | M3 | Na configuração de execução: Permitir várias instâncias (rodar de novo abre outra aba numerada, `server (2)`, sem parar a anterior) e Guardar como arquivo do projeto (`.crabcraft/runConfigurations/*.json`, versionável; ao abrir o projeto o arquivo prevalece sobre a sessão; arquivos quebrados ou grandes são ignorados). Testado com Cargo real |
| B-12 | Configurações compostas | ✅ | M3 | Configuração composta (seletor › Adicionar configuração composta…) aceita binários Cargo: Run inicia todos, cada um na sua aba da janela Run (o Cargo compila um de cada vez); editar e excluir pelo seletor; guardada na sessão e mantida ao recarregar o Cargo. Testado com dois binários reais |
| B-13 | Ícones de run na margem (`fn main`, `#[test]`) | ✅ | M2 | Ícones e clique vistos na janela real |
| B-14 | Seletor de perfil Cargo (dev/release) | ✅ | M3 | Perfil por configuração, na edição da configuração; não há seletor na barra |
| B-15 | Run Anything (Ctrl duplo) | ✅ | M3 | Comandos Cargo, configurações de execução e histórico; só roda Cargo, não comandos do sistema; visto na janela nativa |
| B-16 | Configuração wasm-pack | ⬜ | M4 | |
| B-17 | Backtrace configurável (`RUST_BACKTRACE`) | ✅ | M3 | short, full ou 0 na configuração; o ambiente da configuração tem prioridade |
| B-18 | Links clicáveis em stack traces | ✅ | M2 | Clique simples no console abre o arquivo na linha e coluna; panic real clicado na janela nativa |
| B-19 | Tarefas antes do launch | ✅ | M3 | Configuração Cargo › Antes de executar: ferramentas externas marcadas rodam antes do build, na ordem em que foram marcadas, também ao depurar; a saída vai para a janela Run (ou o console do depurador) e uma que falha (ou some, ou tem macro sem valor) impede a execução com o motivo. Testado com Cargo real; captura conferida. Falta: tarefas que não sejam ferramentas externas |
| B-20 | Configurações temporárias e salvar como permanente | ✅ | M3 | Rodar um teste ou módulo pela margem cria uma configuração temporária (marcada "temporária" no seletor, até 5; as mais antigas saem) que Run roda de novo; Salvar configuração a torna permanente (guardada na sessão) e Excluir a remove; sobrevivem a recarregar o Cargo. Testado com `cargo test` real. Depurar uma configuração de teste ainda não: o seletor avisa e a margem continua depurando |
| B-21 | Pastas de configurações | ✅ | M4 | No seletor de configurações: Mover “x” para pasta (pastas existentes, Nova pasta…, Fora da pasta) e Renomear pasta (as configurações vão junto); cada pasta vira um submenu com as suas configurações. Salvas na sessão (não no arquivo de configuração do projeto). Testado na interface com um projeto de três binários; captura conferida. |
| B-22 | Argumentos após `--` e redirecionar stdin | ✅ | M3 | Argumentos vão direto ao programa; arquivo de entrada testado, ignorado ao depurar |

## 8. Testes e cobertura

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| T-01 | Rodar teste, módulo ou todos pela margem | ✅ | M2 | Teste, módulo e binário pelo ícone, com cliques reais |
| T-02 | Árvore de resultados | ✅ | M2 | Árvore por módulo vista na janela real |
| T-03 | Marcadores de resultado na margem | ✅ | M2 | Passou e falhou vistos na margem |
| T-04 | Rodar novamente os que falharam | ✅ | M2 | Só os que falharam rodam de novo, testado com cargo real |
| T-05 | Ir do erro do teste para o código | ✅ | M2 | Do resultado para a linha do panic, testado com cargo real |
| T-06 | Doctests | ✅ | M2 | Doctests contam na execução; testado com Cargo real |
| T-07 | Runner nextest | ✅ | M3 | Opção em Configurações > Editor; resultados, falha com a linha do panic e rodar de novo os que falharam, testado com cargo-nextest 0.9.150 real. Sem doctests (o nextest não os roda) |
| T-08 | Cobertura de código | ⬜ | M4 | `cargo llvm-cov` |
| T-09 | Suítes de cobertura e importação de arquivos | ⬜ | M4 | |
| T-10 | Depurar um teste | ✅ | M2 | Pelo menu do ícone, parou no breakpoint com GDB real, visto na janela |
| T-11 | Benchmarks pela margem | ✅ | M3 | Ícone na margem roda `cargo bench`, verificado com `harness = false`; benchmarks não são depurados |
| T-12 | Filtrar e ordenar resultados de teste | ✅ | M2 | Filtro por nome e falhas primeiro, vistos na janela |
| T-13 | Marcar falhas de testes no editor após a execução | ✅ | M3 | A mensagem do panic aparece em vermelho na linha da falha e some quando o texto muda |

## 9. Depurador

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| D-01 | Sessão de depuração | ✅ | M2 | `rust-gdb -i=dap`; sessão real testada |
| D-02 | Breakpoints de linha | ✅ | M2 | Testado com GDB real |
| D-03 | Breakpoints condicionais | ✅ | M2 | Condição testada com GDB real |
| D-04 | Breakpoints em exceção/panic | ✅ | M2 | Para na linha do código que causou o panic, com GDB real; também para em should_panic |
| D-05 | Step over, into, out e Run to Cursor | ✅ | M2 | Over, into, out e Run to Cursor testados com GDB real |
| D-06 | Force Step Into | ✅ | M4 | Alt+Shift+F7 (e no menu ⋮ da janela Debug): com GDB, desliga o skip da biblioteca padrão até a próxima parada e entra em `String::from`; com outros adaptadores fica desabilitado, com o motivo. Testado com GDB real. |
| D-07 | Threads e frames | ✅ | M2 | Thread com o nome do teste e pilha vistos na janela real |
| D-08 | Árvore de variáveis | ✅ | M2 | Testado com GDB real, inclusive `Vec` e `String`; registradores ficam fora da árvore |
| D-09 | Watches | ✅ | M2 | Watches testados com GDB real |
| D-10 | Avaliar expressão | ✅ | M2 | Avaliar expressão testado com GDB real |
| D-11 | Valores inline no editor | ✅ | M2 | Valores no fim da linha vistos na janela real |
| D-12 | Definir valor de variável | ✅ | M4 | "Set Value…" (F2, duplo clique ou menu de contexto) em Variables e Watches abre um campo inline com o valor atual; Enter envia, Esc cancela, o erro do depurador aparece sob o campo. Usa `setVariable` ou, sem ele, `setExpression` no `evaluateName` (watches usam sempre `setExpression`); sem nenhum dos dois o item fica desabilitado. Depois de definir, Variables, Watches, valores inline e Memory são atualizados; aceita valor hex (`0x1f`) com hex ligado. Testado num programa Rust real sob GDB 15 (`a_variable_is_set_while_paused_and_the_program_sees_it`): o programa usa o novo valor. Não comparado com o RustRover |
| D-13 | Set execution point | ✅ | M4 | Definir o ponto de execução no cursor (menu Executar, Search Everywhere, menu de passos forçados da janela de depuração; Ctrl+Shift+F10 no mapa Visual Studio). Com GDB: `jump` com breakpoint temporário, só dentro da função atual e só em linha onde começa uma instrução (o GDB, sob DAP, pularia para qualquer lugar sem perguntar); outros adaptadores via `gotoTargets`/`goto`. Testado com GDB num projeto Rust: pular para a frente não executa as linhas puladas, voltar executa de novo, linha de outra função e linha sem código são recusadas com aviso. |
| D-14 | Attach a processo | ✅ | M2 | Attach com GDB real e diálogo visto; com ptrace_scope 1 o Linux bloqueia e o app explica |
| D-15 | Attach automático por padrão de nome | ✅ | M4 | No diálogo Attach to Process: padrões de nome separados por vírgula (`*` e `?`, comparados com o nome do processo e do executável) e Começar/Parar de observar. Uma thread lê a lista de processos a cada 400 ms e o GDB se anexa ao primeiro processo novo que casar, só quando não há sessão de depuração; os que já rodavam quando a observação começou não contam. Os padrões ficam na sessão, mas a observação nunca começa sozinha ao abrir o projeto. Testado com GDB num programa Rust real (o processo antigo é ignorado, o novo é anexado e para no breakpoint, um terceiro durante a depuração é ignorado); captura conferida. Fechado o diálogo, não há indicador de que a observação continua ligada. Com `ptrace_scope` 1, o programa precisa permitir o attach (como no D-14). |
| D-16 | Depuração remota | ⬜ | M6 | `gdbserver` |
| D-17 | Memory view | ✅ | M4 | Aba "Memory" na janela Debug: "Show in Memory View" no menu de contexto de Variables/Watches (usa o `memoryReference` da variável ou o endereço de `&expr`) ou um endereço/expressão digitado (`&x`, `0x7ffd…`). Dump hex com coluna de endereço, 16 bytes por linha e ASCII; bytes ilegíveis aparecem como `??`; páginas de 512 bytes lidas sob demanda (anterior/próxima) com `readMemory`. Relê a página a cada parada e destaca os bytes que mudaram desde a parada anterior. O GDB 15.1 anuncia `supportsReadMemoryRequest` e dá `memoryReference` só para ponteiros (arrays usam `&nome`); quando a leitura falha ele recusa a página inteira, sem leitura parcial. Testado num programa Rust real sob GDB (`the_memory_view_shows_bytes_and_highlights_what_a_step_changed`). Escrever na memória fica fora do escopo |
| D-18 | Disassembly view | ✅ | M4 | Aba "Disassembly" na janela Debug: instruções em volta da instrução atual do frame selecionado (endereço, bytes, instrução) lidas com `disassemble`, com cabeçalho quando muda a função ou a linha do fonte e a instrução atual marcada. Segue o frame selecionado (num chamador mostra o endereço de retorno) e relê a cada parada; "carregar anteriores/seguintes" sob demanda. O GDB 15 não desmonta para trás (`instructionOffset` negativo dá instruções erradas), então as anteriores são lidas de bytes antes do endereço e só aceitas quando a decodificação cai exatamente nele. Step Over/Into Instruction (`next`/`stepIn` com `granularity: instruction`) quando o adaptador anuncia `supportsSteppingGranularity`; adaptador sem `supportsDisassembleRequest` mostra "Este depurador não consegue desmontar". Testado num programa Rust real sob GDB (`the_disassembly_view_marks_the_current_instruction_of_the_function`). Breakpoints em instruções e sintaxe Intel ficam fora do escopo |
| D-19 | Inteiros em hexadecimal | ✅ | M4 | "Show Integers in Hex" no menu de contexto de Variables, no menu Run e no Find Action; vale para Variables, Watches, valores inline, hover e Evaluate, sem reiniciar a sessão, e fica salvo nas configurações. O GDB 15.1 respeita o `ValueFormat { hex }` do DAP (`output-radix 16`, negativos em complemento de dois pela largura do tipo); adaptadores sem esse suporte convertem no cliente. Testado num programa Rust real sob GDB (`integers_switch_to_hexadecimal_while_paused`). Com hex, `char` aparece como número (`0x61`), como o GDB imprime |
| D-20 | Parallel stacks | ⬜ | M4 | |
| D-21 | Escolher GDB ou LLDB | ⬜ | M3 | |
| D-22 | Renderizadores de tipos Rust (`Vec`, `HashMap`, `Rc`…) | ✅ | M2 | Pretty-printers do rustc via `rust-gdb`; testado |
| D-23 | Depurar como root | ⬜ | M6 | |
| D-24 | Pause, resume, restart e stop | ✅ | M2 | Testado com GDB real; botões vistos na janela |
| D-25 | Mute breakpoints | ✅ | M2 | Testado com GDB real; botão visto na janela |
| D-26 | Logpoints e remover ao atingir | ✅ | M3 | Mensagem com `{expressão}` e "Remover ao atingir" nas propriedades do breakpoint; salvos na sessão. Remover ao atingir vale só para breakpoints que param: logpoint não para, então a IDE não vê o acerto. |
| D-27 | Grupos de breakpoints e lista geral | ✅ | M4 | Ver breakpoints… (Ctrl+Shift+F8; ⇧⌘F8 no macOS) lista todos os breakpoints do projeto, inclusive de arquivos fechados, agrupados: ligar/desligar um ou o grupo inteiro, mover para grupo, remover do grupo, renomear/desagrupar, ir para, editar condição/log, remover. Grupos e breakpoints de arquivos fechados ficam na sessão. Testado com GDB num projeto Rust: o breakpoint de um arquivo fechado para o programa, o grupo desligado não para, desligar pela lista durante a sessão vale na hora. Corrigido também: com GDB 15, só os breakpoints do primeiro arquivo valiam (o launch agora espera o configurationDone). Captura conferida. |
| D-28 | Force Step Over e Force Run to Cursor | ✅ | M4 | Alt+Shift+F8 e Ctrl+Alt+F9 (e no menu ⋮ da janela Debug): o adaptador fica sem os breakpoints do usuário (só o do cursor) até o programa parar, e eles voltam em seguida. Testado com GDB real: passa duas vezes por um breakpoint e o Step Over normal volta a parar nele. |
| D-29 | Console do depurador | ✅ | M2 | Comandos chegam ao GDB, testado; caixa vista na janela |
| D-30 | Pular a stdlib ao fazer step | ✅ | M3 | Pula `core`/`alloc`/`std` por nome de função; breakpoints e panics ainda param na biblioteca. Um Step Into pode parar de novo na mesma linha, e closures passadas à biblioteca são puladas. |
| D-31 | Sessões em paralelo | ⬜ | M4 | |

## 10. Profiling e REPL

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| A-01 | Profiler | 🚧 | M4 | Feito: ação Profile (Find Action, menu Run e janela Profiler) compila a configuração de execução Rust selecionada e grava com `perf record` (`cpu-clock:u`, pilhas por DWARF) em segundo plano; Stop and Analyze encerra a gravação e mantém as amostras, Cancel mata o `perf` e o programa (grupo de processos). `perf script` é lido em segundo plano, com limites de amostras, profundidade e nós, e vira flame graph (clique amplia, duplo clique abre o código) e árvore de chamadas com total e self; funções do binário são ligadas ao código-fonte por `nm -l`. Com `kernel.perf_event_paranoid` acima de 2 a janela explica o bloqueio e como liberar. Testado: o caminho real até o `perf` neste computador (paranoid = 4, recusado com a mensagem), cancelamento, o mapeamento por `nm` num binário Rust real e o leitor com saída no formato do `perf script`; capturas conferidas. Falta: verificar uma gravação real (o `perf` não pode gravar aqui sem mudar o sistema), perfis de alocação/memória, comparar perfis, inline frames e informações de linha em builds release sem debug |
| A-02 | Console REPL | ⬜ | M4 | `evcxr_repl` em `rc-term` |
| A-03 | REPL: painel de variáveis | ⬜ | M4 | |
| A-04 | REPL: comando `:dep` | ⬜ | M4 | |

## 11. Controle de versão

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| V-01 | Alterações e commit | 🟢 | M1 | |
| V-02 | Branches | 🟢 | M1 | |
| V-03 | Diff lado a lado | 🟢 | M1 | |
| V-04 | Marcas na margem e blame | 🟢 | M1 | |
| V-05 | Fetch, pull e push | 🟢 | M1 | Confirmar a cobertura |
| V-06 | Log e histórico de arquivo | ✅ | M5 | Log com filtros de texto da mensagem (literal), autor e branch (ou todos), "Carregar mais" depois de 500 commits e mensagem quando nada corresponde. Mostrar histórico do Git (aba do editor e Explorer, também para pastas) abre o Log só daquele caminho, seguindo renomeações. Testado num repositório real; captura conferida. |
| V-07 | Merge, rebase e cherry-pick | ⬜ | M5 | |
| V-08 | Resolução de conflitos em 3 painéis | ⬜ | M5 | |
| V-09 | Stash e shelve | 🚧 | M5 | Feito (stash): Janela Git › aba Stashes lista os stashes (branch, data, mensagem); Guardar alterações com mensagem, arquivos não rastreados e manter o índice; Aplicar, Pop (com Restaurar índice) e Descartar com confirmação; mostra os arquivos de cada stash (inclusive não rastreados) e abre o diff; tudo em segundo plano e conferindo que o stash ainda é o mesmo antes de agir. Testado com repositórios git reais; capturas conferidas. Falta: shelve (prateleiras do IDE), stash de arquivos selecionados e criar branch a partir de um stash |
| V-10 | Changelists | ⬜ | M5 | |
| V-11 | Worktrees | ✅ | M5 | Aba Worktrees na janela Git: lista (atual, branch ou HEAD destacado, bloqueado, podável), adicionar com branch nova, existente ou HEAD destacado (pasta sugerida ao lado do repositório), abrir nesta janela ou em outra, bloquear com motivo/desbloquear, remover (com mudanças locais pede confirmação para forçar; o branch fica) e podar mostrando a saída do Git. Testado com repositórios reais; capturas conferidas. |
| V-12 | Tags | ✅ | M5 | Janela Git › aba Tags: lista (nome, anotada ou leve, commit, data, mensagem), Nova tag em HEAD ou num commit do Log (menu de contexto), leve ou anotada; nome validado com `git check-ref-format`; Checkout (aviso de HEAD destacado), Push de uma tag ou de todas (escolha do remoto, saída do git mostrada) e Excluir, todos com confirmação e em segundo plano. Testado com repositórios git reais e remoto bare; capturas conferidas. Fora: excluir a tag no remoto |
| V-13 | Patches | ✅ | M5 | Criar patch dos arquivos marcados na janela Commit (ou de um arquivo, pelo menu de contexto) e de um ou vários commits do Log (Ctrl/Shift+clique), salvo em arquivo ou copiado; Aplicar patch (janela Git) de arquivo ou da área de transferência, com a lista de arquivos, se aplica sem conflito (`git apply --check`), opção `--3way` e a saída do Git. Patch de e-mail vira commits (`git am`, avisa que os hooks rodam); diff vai para a árvore de trabalho. Limite de 32 MB. Testado entre dois repositórios reais. |
| V-14 | Assinatura GPG | ✅ | M5 | Janela Commit: "Assinar com GPG" começa como diz `commit.gpgsign` e assina com `-S` (a chave de `user.signingkey` aparece na dica); desmarcado usa `--no-gpg-sign`. O Log mostra a assinatura verificada pelo git (boa, não confiável, expirada, revogada, inválida, chave ausente). Falha do programa de assinatura vira erro claro e nada é commitado. Testado com chave GPG real. |
| V-15 | Editar histórico | ⬜ | M5 | |
| V-16 | Integração com issue trackers | ⬜ | M5 | |
| V-17 | GitHub: conta, PRs, revisão, fork, gists, Actions | ⬜ | M5 | Via `gh` |
| V-18 | GitLab: conta, MRs, revisão, snippets, CI/CD | ⬜ | M5 | Via `glab` |

## 12. Ferramentas integradas

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| F-01 | Terminal | 🟢 | M1 | |
| F-02 | Depurar processo a partir do terminal | ⬜ | M6 | |
| F-03 | HTTP Client (arquivos `.http`) | ✅ | M5 | Arquivos `.http` e `.rest` com realce; requisições separadas por `###` (método, URL com linhas `?`/`&` de continuação, cabeçalhos, corpo, nome por `### nome` ou `# @name`); ícone de execução na margem de cada requisição envia em segundo plano (cancelável, tempo limite de 60 s, corpo limitado a 8 MB). Janela HTTP Client: histórico da sessão, status, tempos, tamanho, cabeçalhos e corpo (JSON reindentado sem reordenar), link de volta à requisição; funciona também com arquivo avulso. Variáveis `{{…}}`, `@var =`, `< arquivo`, scripts `> {% %}`, `>> arquivo` e diretivas `# @…` aparecem como avisos no editor e na resposta (variáveis e `< arquivo` impedem o envio). Testado contra servidor HTTP local; capturas conferidas. Fora: tudo de F-04, WebSocket/gRPC/GraphQL, cookies entre requisições, abrir a resposta no editor |
| F-04 | HTTP Client: variáveis, OAuth 2.0, scripts | ⬜ | M5 | |
| F-05 | HTTP Client CLI | ⬜ | M6 | |
| F-06 | Axum: completar URLs e hints de rotas (2026.2) | ⬜ | M5 | |
| F-07 | Janela Endpoints | ⬜ | M5 | |
| F-08 | Gerar chamada Reqwest a partir de endpoint (2026.2) | ⬜ | M5 | |
| F-09 | Docker | ⬜ | M5 | Via CLI `docker` |
| F-10 | Solana (experimental) | 🚫 | - | Plugin de nicho |
| F-11 | LinkedIn Connected Apps | 🚫 | - | Sem valor para o foco em Rust |
| F-12 | Ferramentas externas | ✅ | M3 | Configurações › Ferramentas externas: nome, programa, argumentos (com aspas) e pasta, com macros `$FilePath$`, `$FileDir$`, `$FileName$`, `$FileNameWithoutExtension$`, `$FileRelativePath$`, `$ProjectFileDir$`, `$LineNumber$`, `$ColumnNumber$`, `$SelectedText$`; roda por Mais ações › Ferramentas externas ou Find Action, numa aba da janela Run com Rerun e Stop; macro sem valor avisa antes de rodar. Testado com `wc` real num caminho com espaço; capturas conferidas |
| F-13 | Terminal: abas, divisão e busca | 🟢 | M1 | Conferir o que veio do DotCraft |
| F-14 | Terminal: completar comandos | ⬜ | M6 | |

## 13. Banco de dados, implantação e remoto

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| Y-01 | Conexão a bancos e explorador de esquemas | ⬜ | M6 | Começar por SQLite e Postgres |
| Y-02 | Console SQL e editor de dados | ⬜ | M6 | |
| Y-03 | SQL injetado em strings | ⬜ | M6 | |
| Y-04 | Exportar e importar dados | ⬜ | M6 | |
| Y-05 | Implantação por SFTP/FTP | ⬜ | M6 | |
| Y-06 | Comparar e editar arquivos remotos | ⬜ | M6 | |
| Y-07 | Configurações SSH e terminal SSH | ⬜ | M6 | |
| Y-08 | Desenvolvimento remoto | ⬜ | M6 | |
| Y-09 | Dev Containers | ⬜ | M6 | |
| Y-10 | Colaboração em tempo real | 🚫 | - | Exige serviço próprio |

## 14. Plataforma da IDE

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| G-01 | Layout New UI: ilhas e faixas de tool windows | 🟢 | M1 | |
| G-02 | Tela de boas-vindas e projetos recentes | ✅ | M1 | Abre pasta ou `Cargo.toml`; tela vista no app |
| G-03 | Keymaps | 🟢 | M1 | Rider (macOS e Windows/Linux), Visual Studio e VS Code (macOS e Windows/Linux, atalhos padrão da referência pública do VS Code; acordes de duas etapas como Ctrl+K Ctrl+S ainda não existem). VS Code testado headless: Ctrl+P, Ctrl+Shift+P e Ctrl+D. Falta o keymap do RustRover: o herdado é o do Rider |
| G-04 | Configurar atalhos | ✅ | M3 | Configurações › Keymap › Atalhos: cada ação do Find Action com seu atalho; Gravar atalho captura a próxima tecla com modificadores (Esc cancela), Remover deixa sem atalho e Restaurar volta ao do keymap; o atalho tomado sai da outra ação; guardado nas configurações e aplicado sem reiniciar. Teste headless. Falta: ações de edição fora do Find Action |
| G-05 | Interface em vários idiomas (inclui pt-BR) | 🟢 | M1 | |
| G-06 | Backup e sincronização de configurações | ⬜ | M6 | Exportar/importar arquivo |
| G-07 | Find Action (Ctrl+Shift+A) | ✅ | M3 | Ctrl+Shift+A abre a busca na aba Ações; visto na janela nativa |
| G-08 | Acessibilidade e leitores de tela | ⬜ | M6 | |
| G-09 | Plugins | ⬜ | M6 | |
| G-10 | Migração a partir do VS Code e do RustRover | ⬜ | M6 | |
| G-11 | CLI: abrir arquivos | ✅ | M3 | `crabcraft [--line N] [--column N] arquivo[:linha[:coluna]]…`, com `--help` e `--version` traduzidos; arquivos vão para a janela já aberta (socket do usuário em `$XDG_RUNTIME_DIR` ou na pasta de dados; no Windows porta local com token), pasta ou `Cargo.toml` abre outra janela. Visto na janela nativa: `crabcraft src/main.rs:4:20` pôs o cursor em 4:20 no CrabCraft aberto. Falta `--wait` |
| G-12 | CLI: diff e merge | ⬜ | M6 | |
| G-13 | CLI: formatar e inspecionar | ⬜ | M6 | |
| G-14 | Assistente de IA com modelos locais | ⬜ | M6 | ollama por padrão |
| G-15 | Agentes de IA via ACP | ⬜ | M6 | |
| G-16 | Completar com next edit suggestions | ⬜ | M6 | |

---

## Fontes

Consultadas em 2026-10-31. Os textos acima são resumos nossos.

- [Índice da ajuda do RustRover](https://www.jetbrains.com/help/rust/getting-started.html): 415 tópicos
  (`HelpTOC.json`), com leitura direta das páginas específicas de Rust.
- [Site do RustRover](https://www.jetbrains.com/rust/) e [What's New 2026.2](https://www.jetbrains.com/rust/whatsnew/).
- [Post da versão 2026.2](https://blog.jetbrains.com/rust/2026/07/22/whats-new-in-rustrover-2026-2/)
  e [post sobre Axum](https://blog.jetbrains.com/rust/2026/07/23/axum-support-in-rustrover/):
  usados pelos resumos da busca; o acesso direto ao post da versão retornou 403.

O post completo da 2026.2 não foi lido, então esta lista pode não trazer todas as novidades da versão.

O CrabCraft não tem afiliação com a JetBrains. "RustRover" aparece aqui apenas como referência de
comparação.
