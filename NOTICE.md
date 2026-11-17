# Ordoc: aviso de direitos

Ordoc © 2026 RafaParise.

O código do Ordoc é distribuído sob a **GNU Affero General Public License, versão 3** (`AGPL-3.0-only`), cujo
texto integral está no arquivo `LICENSE`. Em resumo: você pode usar, estudar, modificar e redistribuir; quem
distribuir uma versão modificada, ou oferecê-la a outras pessoas pela rede, precisa disponibilizar o código-fonte
dela sob a mesma licença.

**Licença comercial.** Quem precisar usar o Ordoc, ou partes dele, sem as obrigações da AGPL (por exemplo, dentro
de um produto fechado) pode pedir uma licença comercial ao autor. Contato: Issues do repositório (https://github.com/RafaParise/ordoc-publico/issues).

**Marca.** "Ordoc" e o ícone "O Nó Convergente" não são licenciados pela AGPL: veja `TRADEMARK.md`.

**Independência.** O Ordoc é um projeto pessoal, independente e não oficial, mantido em equipamento próprio. Não é
produto da Secretaria da Educação do Estado de São Paulo nem de nenhum órgão público.

## Componentes de terceiros

O Ordoc usa bibliotecas livres com as próprias licenças (lista completa no `Cargo.lock`). Entre elas:

- Tauri (MIT/Apache-2.0), no app completo. O logotipo do Tauri não é usado.
- OpenSSL (Apache-2.0), compilado junto com o SQLCipher só para a cifra do banco (a rede usa rustls).
- Leptos (MIT).
- SQLCipher (licença estilo BSD da Zetetic, que exige atribuição): "Este produto usa o SQLCipher, da Zetetic LLC".
- rustls, reqwest, tokio, serde e outras (MIT e/ou Apache-2.0).

Antes de cada versão pública, conferir as licenças com `cargo about` ou `cargo deny`, que são ferramentas gratuitas.
