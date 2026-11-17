# Como contribuir com o Ordoc

> **Por enquanto, contribuições de código estão pausadas** até o CLA ser revisado por advogado. Issues (ideias e problemas, sem dado de aluno) são bem-vindas.

Obrigado pelo interesse! Antes de abrir um pull request, leia estas regras.

## 1. O acordo de contribuição (CLA) é obrigatório

O Ordoc é distribuído sob a AGPL-3.0 e o autor pode oferecer, a quem precisar, uma **licença comercial** (veja
`NOTICE.md`). Isso só é possível se o autor tiver direito sobre todo o código, inclusive o das contribuições.

Por isso, toda contribuição de código só é aceita com o **CLA** (`CLA.md`) assinado. Com ele:
- **você continua dono** do que escreveu;
- você dá ao autor uma licença **gratuita, perpétua, irrevogável, mundial e sublicenciável** sobre a sua
  contribuição, que permite distribuí-la também sob outras licenças, inclusive comerciais;
- o Ordoc continua aberto sob a AGPL-3.0 para todo mundo.

Só a declaração DCO (`Signed-off-by`) **não basta**: ela não permite oferecer a contribuição sob outra licença.

## 2. Regras do projeto (resumo)

- 100% Rust. Rede só com `rustls` (nada de OpenSSL na rede). Interface em Leptos, sem Node.js nem Electron.
- Nada de `unwrap()` fora de teste. Erros com `thiserror`.
- Segredo (senha, cookie, token) sempre em tipo que se apaga da memória (`Zeroize`).
- **Nenhum dado de aluno** (nome, RA, nota, falta, texto de resposta) em arquivo, log, teste, issue ou pull request.
  Use dados fictícios.
- Escrita em sistema oficial só com o "sim" do professor por operação; nunca forjar presença, hora, local ou nota.
- Valide com `cargo test --workspace` e `cargo clippy`.

## 3. Segurança

Achou uma falha de segurança? **Não abra issue pública.** Use o aviso privado de vulnerabilidade do GitHub (aba *Security* → *Report a vulnerability*).
