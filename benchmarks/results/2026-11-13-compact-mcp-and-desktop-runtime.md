# Benchmark do catálogo MCP compacto e dos pontos de entrada desktop

As contagens usam `tiktoken` 0.14.0 e `o200k_base`. O catálogo medido é o JSON compacto do resultado de `tools/list`. O custo de inicialização soma catálogo e instruções quando simulamos o contexto do modelo. Também medimos as respostas completas no protocolo. O custo real depende de como cada cliente apresenta e armazena essas definições.

## Perfil MCP compacto

Com `codegraph mcp --tool-profile compact` (`ae7384b`), o código anuncia 11 ferramentas frequentes e `get_tool_schema`. Os testes usaram uma sessão MCP persistente por projeto, com o índice já criado e `--no-watch`, em sete pares após um aquecimento. O perfil padrão e o `full` explícito devolveram as mesmas respostas de inicialização e descoberta do binário anterior.

| Medida por servidor | Full | Compact |
| --- | ---: | ---: |
| Ferramentas anunciadas | 32 | 12 |
| Catálogo | 5.666 | 2.545 |
| Instruções | 125 | 69 |
| Catálogo e instruções | 5.791 | 2.614 |
| Respostas JSON-RPC de inicialização e descoberta | 5.865 | 2.688 |

A inicialização caiu **54,9%** na representação de catálogo e instruções, e **54,2%** no protocolo. O schema de cada ferramenta anunciada coincide com o do perfil completo.

Nos três projetos, `get_file_skeleton` foi descoberto por nome e executado por `batch_query`. O schema foi idêntico ao original e a saída preservou o corpo do resultado. A primeira descoberta acrescentou 146 tokens para obter os nomes e 164 para ler o schema, mais 27 de argumentos. Com o schema já conhecido, o envelope do lote acrescentou 9 tokens à saída e 12 aos argumentos. A inicialização, os argumentos e a saída desse fluxo completo somaram 5.866 → 3.047 tokens em chat-on-steroids, 5.858 → 3.039 em Nodex e 5.990 → 3.171 em LFG.

Com os dois perfis no mesmo binário de release, a mediana da inicialização foi 33,057 → 36,832 ms em chat-on-steroids, 48,559 → 47,827 ms em Nodex e 25,795 → 20,810 ms em LFG. A redução de tokens não teve ganho de tempo consistente nesse teste.

## Skill da CLI

O guia principal de `bcc2279` caiu de 591 para 488 tokens, uma economia de 17,4%. A instrução sobre pontos de entrada desktop levou a versão publicada a 500 tokens. Ele conserva os atalhos usados nos ensaios anteriores, a sintaxe de consulta, a cobertura do índice e o acesso ao catálogo compacto. A referência opcional tem 102 tokens. Carregar os dois arquivos custa 590. As duas contagens são do texto entregue ao agente, com a mesma codificação.

## Pontos de entrada de Electron e Tauri

`get_project_overview(runtime:true)` (`5e804b4`) reúne evidência de inicialização estática em CLI e MCP. Em chat-on-steroids, a visão inclui o entry do pacote, entradas do build, o preload da janela, a ponte de contexto e o script carregado pelo HTML. Em Nodex, distingue o binário desktop, o registro de comandos, a inicialização do servidor embutido e o binário separado. A interface web é mostrada como fonte do build servido, quando a configuração permite verificar a relação. LFG é o controle negativo de aplicação web sem Electron ou Tauri.

A saída de `get_project_overview` sem `runtime:true` foi idêntica à anterior nos três projetos. A visão runtime teve saída igual via CLI e MCP e respeitou o recorte por caminho. Ela pode mostrar evidência parcial quando a configuração é dinâmica ou parte da cadeia está fora do escopo.

Em uma chamada por projeto, com sete pares após aquecimento e 14 pontos de evidência em comum, a visão runtime usou 586 tokens de argumentos e saída ante 1.654 com um batch de buscas por escopo. A medição representa as partes verificadas das perguntas sobre inicialização, e o candidato também retorna evidência de pacote e HTML em chat-on-steroids. Os tokens coincidiram entre CLI e MCP neste teste, que usa um JSON canônico para os argumentos. A consulta runtime custou mais tempo de execução.

| Projeto | Tokens, batch → runtime | CLI, mediana | MCP, mediana |
| --- | ---: | ---: | ---: |
| chat-on-steroids | 679 → 384 | 37,853 → 84,092 ms | 4,037 → 46,859 ms |
| Nodex | 975 → 202 | 39,653 → 56,468 ms | 3,515 → 17,442 ms |

O parâmetro `runtime` adiciona 25 tokens ao catálogo nos dois perfis, que passam a 5.691 e 2.570 tokens. As instruções de inicialização permanecem iguais.

## Ensaio independente com duas IAs

Dois agentes responderam às mesmas perguntas em sessões independentes. A leitura convencional usou buscas e intervalos de arquivo. O braço CodeGraph abriu um servidor MCP compacto persistente por projeto e usou as ferramentas anunciadas. Houve uma sessão por braço e 2.880 arquivos comparados por SHA-256 entre as cópias, fora dos índices.

| Projeto | Revisão |
| --- | --- |
| chat-on-steroids | `7c0bbb64de8bd343f2b57bce3d8f2dce76da5565` |
| Nodex | `22284f34a4f270aadc62de87fb08d78d4f7de2b7` |
| LFG | `efd40913ce9453947da524aeca15ebbccb7fb186` |

| Tarefa | Saída convencional | Saída MCP já aberto | Convencional com argumentos | MCP já aberto com argumentos |
| --- | ---: | ---: | ---: | ---: |
| S1 · Configuração de entrada Electron | 433 | 469 | 530 | 530 |
| S2 · Ponte principal e IPC | 408 | 605 | 497 | 793 |
| S3 · Boot Tauri, servidor e frontend | 2.075 | 899 | 2.256 | 1.077 |
| S4 · API independente e comandos Tauri | 111 | 374 | 169 | 456 |
| S5 · Parser do webhook RevenueCat | 307 | 194 | 348 | 231 |
| S6 · Publicação de configuração administrativa | 1.476 | 371 | 1.555 | 444 |
| S7 · Preload e renderer do overlay de pets | 524 | 490 | 574 | 639 |
| **Total** | **5.334** | **3.402** | **5.929** | **4.170** |

Com as conexões abertas, a saída caiu **36,2%**. O total com argumentos caiu **29,7%**. Na média aritmética dos percentuais por tarefa, o custo subiu **17,0%** para saída e **11,9%** com argumentos. S3 e S6 concentram a economia. O protocolo do benchmark exige ganho nas duas agregações. [Protocolo do benchmark](../README.md)

O braço convencional fez 25 operações. O braço CodeGraph fez 16 consultas e três inicializações, sem carregar a skill e sem ler por fora. O logger registrou zero falhas de operação ou clipping. Dois resultados dentro de um batch tinham truncamento pelo orçamento solicitado e preservaram os pontos necessários à resposta. Com os 7.917 tokens somados das três inicializações no wrapper, o total MCP subiu para **12.087**, **103,9%** acima do convencional. O tempo somado das operações foi 733 ms no convencional, 333 ms no MCP já aberto e 820 ms com inicialização. Os tempos tratam das operações instrumentadas.

As duas IAs acertaram os fatos das sete tarefas. O braço CodeGraph cumpriu a restrição de tipos de arquivo em seis. Na S1, o agente também citou uma linha HTML que a visão runtime retornou. Os fatos solicitados nessa tarefa tinham suporte nos tipos autorizados. Excluir S1 na análise de sensibilidade resulta em 32,6% de economia ponderada com argumentos nas outras seis, com MCP já aberto.

O binário publicado em `5e804b4` reproduziu o texto e os códigos de resultado das 16 consultas MCP registradas e o texto das três inicializações. As consultas somaram 3.402 tokens de saída em ambos os binários.

As capturas detalhadas ficaram em armazenamento local.
