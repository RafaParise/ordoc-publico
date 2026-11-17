# Ordoc (núcleo aberto)

App de código aberto para organizar a rotina do professor (Android e Windows), com os dados **no próprio aparelho**:
banco cifrado, nenhum servidor do projeto, nenhum anúncio, e a correção de atividades usa só a IA do próprio aparelho.

> **App independente, não oficial.** O Ordoc não é da Secretaria da Educação nem de nenhum órgão público. O
> professor entra nos sistemas oficiais com a própria conta, na página oficial; o registro oficial continua sendo o
> do sistema da Secretaria.

## O que está neste repositório
| Pasta | O que é |
|---|---|
| `crates/ordoc-crypto` | Chave-mestra no cofre do aparelho, subchaves por domínio (BLAKE3), XChaCha20-Poly1305, troca de chaves X25519 entre aparelhos, Argon2id e o banco SQLCipher. |
| `crates/ia-nucleo` | Núcleo de IA genérico: roteador local/online, higienizador (nomes, CPF, e-mail e telefone nunca vão a motor online), validação de saída estruturada, memória de receitas. |
| `crates/ordoc-core` | Domínio puro: turmas, frequência, atividades, alertas de risco, pendências e prazos. |
| `crates/ordoc-storage` | Repositórios sobre o SQLCipher, com fila de saída (outbox) versionada. |
| `crates/ordoc-ui` | Interface em Rust (Leptos, WASM), sem Node.js. |
| `docs/` | Site do projeto (GitHub Pages): https://rafaparise.github.io/ordoc-publico |

## O que NÃO está aqui, por enquanto
Os conectores com os sistemas oficiais e o host do aplicativo ficam fechados **por cautela**, até uma avaliação
jurídica. O núcleo aqui é completo e testável sozinho.

## Compilar e testar
```
cargo test --workspace
cd crates/ordoc-ui && trunk build --release
```
(O SQLCipher embutido compila código C: no Windows, use um compilador C, como o MSVC ou o llvm-mingw.)

## Licença
AGPL-3.0-only (`LICENSE`). Licença comercial sob consulta (`NOTICE.md`). O nome e o ícone não fazem parte da licença
(`TRADEMARK.md`). Contribuições: `CONTRIBUTING.md` e `CLA.md`. Apoio: `FUNDING.md`.
