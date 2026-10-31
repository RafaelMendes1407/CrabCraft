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
| ✅ Pronto | 75 |
| 🚧 Em andamento | 3 |
| 🟢 Herdado | 23 |
| 🔁 Portar | 3 |
| ⬜ A fazer | 148 |
| 🚫 Fora do escopo | 5 |
| **Total** | **257** |
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
| P-02 | Instalar rustup e stdlib pela IDE | ⬜ | M3 | Executar o instalador oficial com confirmação |
| P-03 | Avisar e aplicar atualização da toolchain | ⬜ | M3 | `rustup check` / `rustup update` |
| P-04 | Toolchain WSL | ⬜ | M6 | |
| P-05 | Toolchain Ferrocene (`criticalup.toml`) | ⬜ | M6 | |
| P-06 | Variáveis de ambiente do projeto | ⬜ | M3 | Aplicadas a cargo e ao rust-analyzer |
| P-07 | Novo projeto: Binary e Library | ✅ | M1 | `cargo new` com Binary e Library; criação testada com Cargo real |
| P-08 | Novo projeto: WebAssembly e Procedural Macro | ⬜ | M4 | |
| P-09 | Modelos cargo-generate (repositório Git) | ⬜ | M4 | |
| P-10 | Configuração de run criada junto com o projeto | ✅ | M1 | Projeto novo abre com a config já selecionada e roda; teste real |
| P-11 | Janela Project (árvore de arquivos e crates) | ✅ | M1 | Árvore de arquivos e Bibliotecas externas (stdlib e crates do `Cargo.lock`, somente leitura); visto na janela nativa com `serde` |
| P-12 | Dependências externas na árvore do projeto | ✅ | M3 | Nó Bibliotecas externas, ver P-11 |
| P-13 | Novo arquivo Rust com anexar ao módulo (`mod x;`) | 🔁 | M3 | Substitui `item_templates` |
| P-14 | Novo módulo (arquivo/diretório, público/privado) | ⬜ | M3 | |
| P-15 | Novo crate Cargo no workspace | ⬜ | M3 | |
| P-16 | Aviso de arquivo fora de módulo, com anexar | ⬜ | M3 | |
| P-17 | Janela Cargo: membros e targets | ✅ | M3 | Lê `cargo metadata`, com recarregar; vista na janela nativa |
| P-18 | Executar target com duplo clique | ✅ | M3 | bin na janela Run; lib, example, test e bench no console de Compilar; `lib` clicado na janela nativa, demais por teste real |
| P-19 | Executar comando Cargo pela janela Cargo | ✅ | M3 | Campo com Enter; `cargo` no início é opcional; falha mostra o código de saída |
| P-20 | Anexar/desanexar projetos Cargo | ⬜ | M3 | |
| P-21 | Recarregar modelo ao mudar `Cargo.toml` (automático) | ✅ | M1 | Editar Cargo.toml recarrega os alvos, testado com cargo real |
| P-22 | Botão Load Cargo Changes (manual) | ✅ | M3 | Botão recarregar na janela Cargo; alterar `Cargo.toml` também recarrega sozinho |
| P-23 | Aba Sync com o log do carregamento | ⬜ | M3 | |
| P-24 | Excluir pastas e arquivos da análise | ⬜ | M3 | |
| P-25 | Invalidar caches | ⬜ | M3 | |
| P-26 | Segurança: projeto confiável antes de rodar build scripts | ✅ | M1 | rust-analyzer só inicia após confiar; teste prova que `build.rs` não roda antes |
| P-27 | Estilo de código compartilhado no projeto | ⬜ | M3 | `rustfmt.toml`, `.editorconfig` |
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
| E-09 | Sticky lines | ⬜ | M3 | |
| E-10 | Zoom do editor e da IDE | 🟢 | M1 | |
| E-11 | Temas claro/escuro e esquemas de cores | 🟢 | M1 | |
| E-12 | Salvar e reverter alterações | 🟢 | M1 | |
| E-13 | Local History | ⬜ | M3 | |
| E-14 | LightEdit (arquivo sem projeto) | ⬜ | M3 | |
| E-15 | Corretor ortográfico e gramática | ⬜ | M6 | |
| E-16 | Live templates e variáveis | ⬜ | M3 | Reaproveitar snippets do LSP |
| E-17 | Postfix completion (`.if`, `.match`…) | ✅ | M3 | Postfix chega ao completar, testado com rust-analyzer real |
| E-18 | Pré-visualização de Markdown | 🟢 | M1 | |
| E-19 | Suporte a JSON e XML | 🟢 | M1 | |
| E-20 | Suporte a HTML, CSS, JavaScript e TypeScript | ⬜ | M6 | Realce primeiro |
| E-21 | Emmet | 🚫 | - | Depende do suporte web |
| E-22 | Cabeçalhos de copyright | ⬜ | M6 | |
| E-23 | Breadcrumbs | ⬜ | M3 | |
| E-24 | Marcas de erro na barra de rolagem | ⬜ | M3 | |
| E-25 | Lens mode na barra de rolagem | ⬜ | M6 | |
| E-26 | Abas: fixar, dividir, destacar, pré-visualização | 🟢 | M1 | Divisão herdada; conferir o restante |
| E-27 | Scratch files | ⬜ | M3 | |

## 3. Inteligência de código

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| I-01 | Análise do projeto (índice) | ✅ | M1 | rust-analyzer com estado de análise na barra de status; testado e visto no app |
| I-02 | Autocompletar com documentação | ✅ | M1 | Testado com rust-analyzer real |
| I-03 | Completar em `#[derive]`, macros e doctests | 🚧 | M1 | Derive e macro testados com rust-analyzer real; em doctest ele não trouxe sugestões na sonda, exige solução própria |
| I-04 | Auto-import ao completar | ✅ | M1 | Import ao aceitar o item testado com rust-analyzer real |
| I-05 | Auto-import ao digitar e ao colar | ⬜ | M3 | |
| I-06 | Excluir itens do auto-import | ⬜ | M3 | |
| I-07 | Adicionar crate popular ao colar código | ⬜ | M4 | |
| I-08 | Sugerir crates ainda não adicionados | ⬜ | M4 | |
| I-09 | Ordenação de completar por ML | 🚫 | - | Sem modelo próprio; rust-analyzer ordena |
| I-10 | Hover e documentação rápida | ✅ | M1 | Hover testado com rust-analyzer real |
| I-11 | Abrir documentação em docs.rs | ⬜ | M3 | |
| I-12 | Informação de parâmetros | ✅ | M1 | Assinatura testada com rust-analyzer real |
| I-13 | Inlay hints (tipos, parâmetros, chaining) | ✅ | M1 | Tipos e parâmetros testados com rust-analyzer real |
| I-14 | Diagnósticos em tempo real | ✅ | M1 | Testado com rust-analyzer real: erro de sintaxe ao digitar e erro do cargo check ao salvar |
| I-15 | Quick fixes e intention actions (Alt+Enter) | ✅ | M1 | Import pelo Alt+Enter testado com rust-analyzer real |
| I-16 | Aplicar correção sugerida pelo compilador | ⬜ | M3 | Campo `suggested_replacement` do rustc |
| I-17 | Inspeções em todo o projeto | ⬜ | M3 | |
| I-18 | Perfis e severidade de inspeções | ⬜ | M4 | |
| I-19 | Suprimir inspeções (`#[allow]`) | ⬜ | M3 | |
| I-20 | Linters externos: Cargo Check e Clippy | ✅ | M3 | `check.command` do rust-analyzer, trocado sem reiniciar; lint `len_zero` do Clippy recebido em teste real |
| I-21 | Widget do linter na barra de status | ✅ | M3 | Clique alterna Cargo Check/Clippy e salva a escolha; visto na janela nativa |
| I-22 | Inspeções de `Cargo.toml` | ⬜ | M4 | |
| I-23 | Visualizar lifetimes em erros do borrow checker | ⬜ | M4 | A partir dos spans do rustc |
| I-24 | Explicações inline de erros | ⬜ | M4 | |
| I-25 | Expandir macro (recursivo e um passo) | ⬜ | M4 | `rust-analyzer/expandMacro` |
| I-26 | Testador interativo de macros declarativas (2026.2) | ⬜ | M4 | |
| I-27 | Reformatar com rustfmt | ✅ | M3 | Reformat Code via rust-analyzer testado |
| I-28 | rustfmt ao salvar e ao commitar | ✅ | M3 | Ao salvar (Configurações > Editor) e ao commitar (janela Commit); só os arquivos incluídos, e um erro de sintaxe cancela o commit sem alterar nada |
| I-29 | Configurar rustfmt (argumentos, canal, env) | ⬜ | M3 | |
| I-30 | Formatador embutido para trechos | ⬜ | M3 | `textDocument/rangeFormatting` |
| I-31 | Completar linha inteira local | ⬜ | M6 | Pode usar ollama |
| I-32 | Otimizar imports | 🚧 | M3 | Remover imports não usados testado; merge falta |
| I-33 | Pré-visualizar intention antes de aplicar | ⬜ | M4 | |

## 4. Navegação e busca

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| N-01 | Search Everywhere (Shift duplo) | 🟢 | M1 | Trocar a parte de símbolos para LSP |
| N-02 | Ir para arquivo, tipo e símbolo | ✅ | M1 | workspace/symbol testado com rust-analyzer real |
| N-03 | Ir para definição | ✅ | M1 | Testado com rust-analyzer real, abre o arquivo da biblioteca |
| N-04 | Ir para implementações | ✅ | M1 | Implementações de trait testadas com rust-analyzer real |
| N-05 | Encontrar usos | ✅ | M1 | Usos testados com rust-analyzer real |
| N-06 | Find in Files e Replace in Files | 🟢 | M1 | |
| N-07 | Arquivos e locais recentes | 🟢 | M1 | |
| N-08 | Estrutura do arquivo | ✅ | M1 | documentSymbol testado com rust-analyzer real |
| N-09 | Hierarquia de chamadas | ⬜ | M3 | `callHierarchy/*` |
| N-10 | Hierarquia de tipos e traits | ⬜ | M3 | |
| N-11 | Ir para o trait / para o `impl` | ⬜ | M3 | |
| N-12 | Bookmarks | ⬜ | M3 | |
| N-13 | Navegar entre features e `cfg` | ⬜ | M4 | |
| N-14 | Navegar para a fonte de dependências | ⬜ | M3 | |
| N-15 | Scopes e cores de arquivo | ⬜ | M4 | |
| N-16 | Navigation bar e Select In | ⬜ | M3 | |
| N-17 | Mostrar contexto do código (Alt+Q) | ⬜ | M4 | |
| N-18 | Compartilhar no Rust Playground | ⬜ | M4 | Pede confirmação antes de enviar o código |

## 5. Refatoração e geração de código

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| R-01 | Lista de refatorações (Ctrl+Alt+Shift+T) | ⬜ | M3 | |
| R-02 | Renomear em todo o projeto | ✅ | M1 | Rename entre arquivos testado com rust-analyzer real |
| R-03 | Pré-visualização de refatoração | ⬜ | M3 | |
| R-04 | Lista de conflitos da refatoração | ⬜ | M3 | |
| R-05 | Extrair variável, constante, função | ✅ | M3 | Extrair variável e função testados com rust-analyzer real |
| R-06 | Inline de variável e função | ✅ | M3 | Inline de variável testado com rust-analyzer real |
| R-07 | Mudar assinatura | ⬜ | M4 | |
| R-08 | Mover itens entre módulos | ⬜ | M4 | |
| R-09 | Popup Generate (Alt+Insert) | 🚧 | M3 | Gerar new pelo Alt+Enter testado; popup Alt+Insert falta |
| R-10 | Implementar membros de trait | ✅ | M3 | Implementar membros de trait testado com rust-analyzer real |
| R-11 | Gerar `derive` | ⬜ | M3 | |

## 6. Cargo.toml e conditional compilation

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| C-01 | Completar chaves do manifesto | ⬜ | M4 | |
| C-02 | Completar nome e versão de crates (crates.io) | ⬜ | M4 | Índice esparso; porta o cliente NuGet |
| C-03 | Mostrar versão do lock e a mais recente | ⬜ | M4 | |
| C-04 | Avisar crates inexistentes ou desatualizados | ⬜ | M4 | |
| C-05 | Gerenciador visual de dependências | 🔁 | M4 | Reescrever a janela NuGet com `cargo add`/`remove` |
| C-06 | `cargo tree` integrado | ⬜ | M4 | |
| C-07 | Ligar/desligar features pela margem | ⬜ | M4 | `rust-analyzer.cargo.features` |
| C-08 | Atenuar código desativado por `cfg` | ⬜ | M4 | |
| C-09 | Seletor de target (triple) na barra de status | ⬜ | M4 | |
| C-10 | Quick fix para habilitar feature | ⬜ | M4 | |
| C-11 | Ícone da margem para abrir docs.rs do crate | ⬜ | M4 | |

## 7. Build e execução

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| B-01 | Build, rebuild e clean | ✅ | M1 | `cargo build` com JSON, rebuild, clean e fetch; build testado com Cargo real |
| B-02 | Janela Build com erros clicáveis | ✅ | M1 | Erro real de compilação clicado na janela nativa abriu o arquivo na linha e coluna |
| B-03 | Botão Apply fix em erros do compilador | ⬜ | M3 | |
| B-04 | Janela Problems | ✅ | M1 | Vista na janela nativa com erro real, também de arquivo fechado; clique na lista só por código herdado, sem teste |
| B-05 | Configurações de run automáticas por target | ✅ | M1 | Uma por binário, mais All Tests; testado e visto no app |
| B-06 | Editor de configurações de run | ✅ | M3 | Diálogo visto na janela real |
| B-07 | Configuração Cargo: comando, canal, env, cwd, stdin | ✅ | M3 | Editar configuração: perfil, toolchain, features, ambiente, pasta e arquivo de entrada, testados no toolchain real, exceto a troca de toolchain (só os argumentos); outros comandos pela janela Cargo |
| B-08 | Features automáticas (`required-features`) | ✅ | M3 | `required-features` lidas do metadata e aplicadas sozinhas; teste real |
| B-09 | Emular terminal na saída | ⬜ | M3 | `rc-term` já suporta |
| B-10 | Executar como root | ⬜ | M6 | |
| B-11 | Múltiplas instâncias e configuração no projeto | ⬜ | M3 | |
| B-12 | Configurações compostas | 🔁 | M3 | |
| B-13 | Ícones de run na margem (`fn main`, `#[test]`) | ✅ | M2 | Ícones e clique vistos na janela real |
| B-14 | Seletor de perfil Cargo (dev/release) | ✅ | M3 | Perfil por configuração, na edição da configuração; não há seletor na barra |
| B-15 | Run Anything (Ctrl duplo) | ⬜ | M3 | |
| B-16 | Configuração wasm-pack | ⬜ | M4 | |
| B-17 | Backtrace configurável (`RUST_BACKTRACE`) | ✅ | M3 | short, full ou 0 na configuração; o ambiente da configuração tem prioridade |
| B-18 | Links clicáveis em stack traces | ✅ | M2 | Clique simples no console abre o arquivo na linha e coluna; panic real clicado na janela nativa |
| B-19 | Tarefas antes do launch | ⬜ | M3 | |
| B-20 | Configurações temporárias e salvar como permanente | ⬜ | M3 | |
| B-21 | Pastas de configurações | ⬜ | M4 | |
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
| T-07 | Runner nextest | ⬜ | M3 | |
| T-08 | Cobertura de código | ⬜ | M4 | `cargo llvm-cov` |
| T-09 | Suítes de cobertura e importação de arquivos | ⬜ | M4 | |
| T-10 | Depurar um teste | ✅ | M2 | Pelo menu do ícone, parou no breakpoint com GDB real, visto na janela |
| T-11 | Benchmarks pela margem | ⬜ | M3 | `cargo bench` |
| T-12 | Filtrar e ordenar resultados de teste | ✅ | M2 | Filtro por nome e falhas primeiro, vistos na janela |
| T-13 | Marcar falhas de testes no editor após a execução | ⬜ | M3 | |

## 9. Depurador

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| D-01 | Sessão de depuração | ✅ | M2 | `rust-gdb -i=dap`; sessão real testada |
| D-02 | Breakpoints de linha | ✅ | M2 | Testado com GDB real |
| D-03 | Breakpoints condicionais | ✅ | M2 | Condição testada com GDB real |
| D-04 | Breakpoints em exceção/panic | ✅ | M2 | Para na linha do código que causou o panic, com GDB real; também para em should_panic |
| D-05 | Step over, into, out e Run to Cursor | ✅ | M2 | Over, into, out e Run to Cursor testados com GDB real |
| D-06 | Force Step Into | ⬜ | M4 | |
| D-07 | Threads e frames | ✅ | M2 | Thread com o nome do teste e pilha vistos na janela real |
| D-08 | Árvore de variáveis | ✅ | M2 | Testado com GDB real, inclusive `Vec` e `String`; registradores ficam fora da árvore |
| D-09 | Watches | ✅ | M2 | Watches testados com GDB real |
| D-10 | Avaliar expressão | ✅ | M2 | Avaliar expressão testado com GDB real |
| D-11 | Valores inline no editor | ✅ | M2 | Valores no fim da linha vistos na janela real |
| D-12 | Definir valor de variável | ⬜ | M4 | |
| D-13 | Set execution point | ⬜ | M4 | |
| D-14 | Attach a processo | ✅ | M2 | Attach com GDB real e diálogo visto; com ptrace_scope 1 o Linux bloqueia e o app explica |
| D-15 | Attach automático por padrão de nome | ⬜ | M4 | |
| D-16 | Depuração remota | ⬜ | M6 | `gdbserver` |
| D-17 | Memory view | ⬜ | M4 | |
| D-18 | Disassembly view | ⬜ | M4 | |
| D-19 | Inteiros em hexadecimal | ⬜ | M4 | |
| D-20 | Parallel stacks | ⬜ | M4 | |
| D-21 | Escolher GDB ou LLDB | ⬜ | M3 | |
| D-22 | Renderizadores de tipos Rust (`Vec`, `HashMap`, `Rc`…) | ✅ | M2 | Pretty-printers do rustc via `rust-gdb`; testado |
| D-23 | Depurar como root | ⬜ | M6 | |
| D-24 | Pause, resume, restart e stop | ✅ | M2 | Testado com GDB real; botões vistos na janela |
| D-25 | Mute breakpoints | ✅ | M2 | Testado com GDB real; botão visto na janela |
| D-26 | Logpoints e remover ao atingir | ⬜ | M3 | |
| D-27 | Grupos de breakpoints e lista geral | ⬜ | M4 | |
| D-28 | Force Step Over e Force Run to Cursor | ⬜ | M4 | |
| D-29 | Console do depurador | ✅ | M2 | Comandos chegam ao GDB, testado; caixa vista na janela |
| D-30 | Pular a stdlib ao fazer step | ⬜ | M3 | |
| D-31 | Sessões em paralelo | ⬜ | M4 | |

## 10. Profiling e REPL

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| A-01 | Profiler | ⬜ | M4 | `perf` + flamegraph. Citado no site; sem seção própria no índice da ajuda |
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
| V-06 | Log e histórico de arquivo | ⬜ | M5 | |
| V-07 | Merge, rebase e cherry-pick | ⬜ | M5 | |
| V-08 | Resolução de conflitos em 3 painéis | ⬜ | M5 | |
| V-09 | Stash e shelve | ⬜ | M5 | |
| V-10 | Changelists | ⬜ | M5 | |
| V-11 | Worktrees | ⬜ | M5 | |
| V-12 | Tags | ⬜ | M5 | |
| V-13 | Patches | ⬜ | M5 | |
| V-14 | Assinatura GPG | ⬜ | M5 | |
| V-15 | Editar histórico | ⬜ | M5 | |
| V-16 | Integração com issue trackers | ⬜ | M5 | |
| V-17 | GitHub: conta, PRs, revisão, fork, gists, Actions | ⬜ | M5 | Via `gh` |
| V-18 | GitLab: conta, MRs, revisão, snippets, CI/CD | ⬜ | M5 | Via `glab` |

## 12. Ferramentas integradas

| ID | Funcionalidade | Status | Marco | Plano |
| --- | --- | --- | --- | --- |
| F-01 | Terminal | 🟢 | M1 | |
| F-02 | Depurar processo a partir do terminal | ⬜ | M6 | |
| F-03 | HTTP Client (arquivos `.http`) | ⬜ | M5 | |
| F-04 | HTTP Client: variáveis, OAuth 2.0, scripts | ⬜ | M5 | |
| F-05 | HTTP Client CLI | ⬜ | M6 | |
| F-06 | Axum: completar URLs e hints de rotas (2026.2) | ⬜ | M5 | |
| F-07 | Janela Endpoints | ⬜ | M5 | |
| F-08 | Gerar chamada Reqwest a partir de endpoint (2026.2) | ⬜ | M5 | |
| F-09 | Docker | ⬜ | M5 | Via CLI `docker` |
| F-10 | Solana (experimental) | 🚫 | - | Plugin de nicho |
| F-11 | LinkedIn Connected Apps | 🚫 | - | Sem valor para o foco em Rust |
| F-12 | Ferramentas externas | ⬜ | M3 | |
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
| G-03 | Keymaps | 🟢 | M1 | Adicionar o keymap do RustRover: o herdado é o do Rider |
| G-04 | Configurar atalhos | ⬜ | M3 | |
| G-05 | Interface em vários idiomas (inclui pt-BR) | 🟢 | M1 | |
| G-06 | Backup e sincronização de configurações | ⬜ | M6 | Exportar/importar arquivo |
| G-07 | Find Action (Ctrl+Shift+A) | ⬜ | M3 | |
| G-08 | Acessibilidade e leitores de tela | ⬜ | M6 | |
| G-09 | Plugins | ⬜ | M6 | |
| G-10 | Migração a partir do VS Code e do RustRover | ⬜ | M6 | |
| G-11 | CLI: abrir arquivos | ⬜ | M3 | |
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
