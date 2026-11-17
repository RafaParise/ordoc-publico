//! Roteamento online/offline (desenho, seção 4.1).
//!
//! - Tarefa `Sensivel` (precisa do dado real): só motores locais, com o texto como veio. Nenhum motor
//!   online é sequer chamado.
//! - Tarefa `Estreita` (rótulo, escolha, campos): locais primeiro; se nenhum local está de pé ou todos
//!   falham, online.
//! - `TextoLivre`: online primeiro, se de pé; senão local, com `modo_offline = true` ("texto mais simples").
//! - Em cada motor: falha HTTP/timeout -> próximo motor na hora; saída fora do esquema -> nova tentativa
//!   no mesmo motor levando o erro, até `Politica::tentativas_por_motor` (2) vezes; depois, próximo motor.
//!
//! Garantia de privacidade: TODO texto que vai a motor `Online` (mensagens do pedido, a resposta anterior
//! do modelo e a mensagem de correção) passa pelo `Higienizador` antes. Motor local também recebe texto
//! higienizado por padrão (`Politica::higienizar_local`), menos na tarefa `Sensivel`.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use crate::error::{ErroIa, Result};
use crate::esquema::{mensagem_correcao, validar_saida, FalhaEsquema};
use crate::higiene::{Higienizador, MapaMarcadores};
use crate::motor::{Alcance, Mensagem, Motor, Papel, PedidoGeracao};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoTarefa {
    /// Exige o dado real (ex.: preencher nomes): só local, nunca online.
    Sensivel,
    /// Rótulo, escolha ou campos com esquema: local primeiro.
    Estreita,
    /// Redação livre: online primeiro, se de pé.
    TextoLivre,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Politica {
    /// Quantas vezes o mesmo motor tenta quando a saída sai do esquema (desenho: 2).
    pub tentativas_por_motor: u32,
    /// Motor local também recebe o texto higienizado (padrão do desenho, seção 7, item 3).
    pub higienizar_local: bool,
}

impl Default for Politica {
    fn default() -> Self {
        Self {
            tentativas_por_motor: 2,
            higienizar_local: true,
        }
    }
}

/// O que aparece no topo da tela.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoMotores {
    Online,
    Local,
    Nenhum,
}

/// Resposta validada. `texto` está como o modelo devolveu (com marcadores); use `restaurar()` no fim,
/// só no aparelho, para trocar os marcadores pelos valores.
pub struct Resposta<T> {
    pub valor: T,
    pub texto: String,
    pub motor: String,
    pub alcance: Alcance,
    /// Texto livre respondido por motor local (avisar "modo offline: texto mais simples").
    pub modo_offline: bool,
    /// Chamadas feitas ao todo (todos os motores) até esta resposta.
    pub tentativas: u32,
    pub mapa: MapaMarcadores,
}

impl<T> std::fmt::Debug for Resposta<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resposta")
            .field("motor", &self.motor)
            .field("alcance", &self.alcance)
            .field("modo_offline", &self.modo_offline)
            .field("tentativas", &self.tentativas)
            .field("mapa", &self.mapa)
            .finish_non_exhaustive()
    }
}

impl<T> Resposta<T> {
    /// O texto da resposta com os marcadores trocados pelos valores originais.
    pub fn restaurar(&self) -> Zeroizing<String> {
        self.mapa.restaurar(&self.texto)
    }
}

/// Escolhe o motor pela política e troca sozinho quando um falha.
pub struct Roteador {
    locais: Vec<Arc<dyn Motor>>,
    online: Vec<Arc<dyn Motor>>,
    higienizador: Higienizador,
    politica: Politica,
}

impl std::fmt::Debug for Roteador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids = |v: &[Arc<dyn Motor>]| v.iter().map(|m| m.perfil().id.clone()).collect::<Vec<_>>();
        f.debug_struct("Roteador")
            .field("locais", &ids(&self.locais))
            .field("online", &ids(&self.online))
            .field("higienizador", &self.higienizador)
            .field("politica", &self.politica)
            .finish()
    }
}

impl Roteador {
    pub fn novo(higienizador: Higienizador) -> Self {
        Self {
            locais: Vec::new(),
            online: Vec::new(),
            higienizador,
            politica: Politica::default(),
        }
    }

    /// Acrescenta um motor na lista certa pelo `perfil().alcance` (não dá para pôr online como local).
    pub fn com_motor(mut self, motor: Arc<dyn Motor>) -> Self {
        match motor.perfil().alcance {
            Alcance::Local => self.locais.push(motor),
            Alcance::Online => self.online.push(motor),
        }
        self
    }

    pub fn com_politica(mut self, politica: Politica) -> Self {
        self.politica = politica;
        self
    }

    pub fn higienizador(&self) -> &Higienizador {
        &self.higienizador
    }

    /// Para atualizar a lista de nomes em tempo de execução.
    pub fn higienizador_mut(&mut self) -> &mut Higienizador {
        &mut self.higienizador
    }

    /// Online se algum online está de pé; senão Local se algum local está; senão Nenhum.
    pub async fn estado(&self) -> EstadoMotores {
        for m in &self.online {
            if m.disponivel().await {
                return EstadoMotores::Online;
            }
        }
        for m in &self.locais {
            if m.disponivel().await {
                return EstadoMotores::Local;
            }
        }
        EstadoMotores::Nenhum
    }

    fn ordem(&self, tarefa: TipoTarefa) -> Vec<Arc<dyn Motor>> {
        match tarefa {
            TipoTarefa::Sensivel => self.locais.clone(),
            TipoTarefa::Estreita => self.locais.iter().chain(&self.online).cloned().collect(),
            TipoTarefa::TextoLivre => self.online.iter().chain(&self.locais).cloned().collect(),
        }
    }

    fn higienizar_pedido(&self, pedido: &PedidoGeracao, mapa: &mut MapaMarcadores) -> PedidoGeracao {
        let mut limpo = pedido.clone();
        for m in &mut limpo.mensagens {
            m.conteudo = self.higienizador.higienizar_com(&m.conteudo, mapa);
        }
        limpo
    }

    /// Geração de texto sem esquema obrigatório (se o pedido trouxer esquema, ele vai ao motor, mas a
    /// saída não é validada).
    pub async fn gerar_texto(
        &self,
        tarefa: TipoTarefa,
        pedido: &PedidoGeracao,
    ) -> Result<Resposta<String>> {
        self.executar(tarefa, pedido, |t: &str| Ok(t.to_owned())).await
    }

    /// Geração com a saída validada contra `T` (serde). Fora do esquema = nova tentativa com o erro.
    pub async fn gerar_estruturado<T: DeserializeOwned>(
        &self,
        tarefa: TipoTarefa,
        pedido: &PedidoGeracao,
    ) -> Result<Resposta<T>> {
        self.executar(tarefa, pedido, validar_saida::<T>).await
    }

    async fn executar<T, F>(
        &self,
        tarefa: TipoTarefa,
        pedido: &PedidoGeracao,
        validar: F,
    ) -> Result<Resposta<T>>
    where
        F: Fn(&str) -> std::result::Result<T, FalhaEsquema>,
    {
        let mut falhas_esquema = 0u32;
        let mut outros_erros = 0u32;
        let mut tentativas = 0u32;
        let por_motor = self.politica.tentativas_por_motor.max(1);

        for motor in self.ordem(tarefa) {
            let alcance = motor.perfil().alcance;
            // Defesa em profundidade: a ordem já não inclui online em tarefa sensível.
            if tarefa == TipoTarefa::Sensivel && alcance == Alcance::Online {
                continue;
            }
            if !motor.disponivel().await {
                continue;
            }
            let higienizar = match alcance {
                Alcance::Online => true,
                Alcance::Local => tarefa != TipoTarefa::Sensivel && self.politica.higienizar_local,
            };

            let mut mapa = MapaMarcadores::default();
            let base = if higienizar {
                self.higienizar_pedido(pedido, &mut mapa)
            } else {
                pedido.clone()
            };
            let mut atual = base.clone();

            for _ in 0..por_motor {
                tentativas += 1;
                let geracao = match motor.gerar(&atual).await {
                    Ok(g) => g,
                    Err(_) => {
                        // Falha HTTP, timeout ou resposta quebrada: próximo motor na hora.
                        outros_erros += 1;
                        break;
                    }
                };
                match validar(&geracao.texto) {
                    Ok(valor) => {
                        return Ok(Resposta {
                            valor,
                            texto: geracao.texto,
                            motor: geracao.motor,
                            alcance,
                            modo_offline: tarefa == TipoTarefa::TextoLivre
                                && alcance == Alcance::Local,
                            tentativas,
                            mapa,
                        });
                    }
                    Err(falha) => {
                        falhas_esquema += 1;
                        let mut anterior = geracao.texto;
                        let mut correcao = mensagem_correcao(&falha);
                        if higienizar {
                            anterior = self.higienizador.higienizar_com(&anterior, &mut mapa);
                            correcao = self.higienizador.higienizar_com(&correcao, &mut mapa);
                        }
                        atual = base.clone();
                        atual.mensagens.push(Mensagem::nova(Papel::Assistente, anterior));
                        atual.mensagens.push(Mensagem::nova(Papel::Usuario, correcao));
                    }
                }
            }
        }

        if outros_erros == 0 && falhas_esquema > 0 {
            Err(ErroIa::SaidaForaDoEsquema {
                tentativas: falhas_esquema,
            })
        } else {
            Err(ErroIa::SemMotorDisponivel)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::higiene::tests::cpf_de_teste;
    use crate::motor::{MotorFalso, RespostaFalsa};
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Rotulo {
        intencao: String,
    }

    fn hig() -> Higienizador {
        Higienizador::novo()
            .unwrap()
            .com_nomes(["Joana Teste Silva", "Beto Inventado"])
    }

    fn texto(s: &str) -> RespostaFalsa {
        RespostaFalsa::Texto(s.to_owned())
    }

    fn todo_texto(p: &PedidoGeracao) -> String {
        p.mensagens
            .iter()
            .map(|m| m.conteudo.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn texto_nao_higienizado_nunca_chega_ao_motor_online() {
        let cpf = cpf_de_teste([4, 0, 2, 7, 1, 8, 2, 8, 1]);
        let originais = [
            "Joana Teste Silva",
            "joana teste silva",
            "BETO INVENTADO",
            "joana.inventada@exemplo.test",
            cpf.as_str(),
            "123456789012",
            "98765-4321",
        ];
        let online = Arc::new(MotorFalso::novo("online", Alcance::Online).com_respostas([
            // 1ª resposta fora do esquema e ecoando um nome: a correção também precisa ir higienizada.
            texto("Beto Inventado? não sei"),
            texto(r#"{"intencao":"carta para [ALUNO_1]"}"#),
        ]));
        let r = Roteador::novo(hig()).com_motor(online.clone());
        let pedido = PedidoGeracao::novo(format!(
            "Escreva para Joana Teste Silva (joana.inventada@exemplo.test, CPF {cpf}, RA 123456789012, \
             tel 98765-4321) e também para joana teste silva e BETO INVENTADO."
        ))
        .com_sistema("Você ajuda Joana Teste Silva.");

        let resp: Resposta<Rotulo> = r
            .gerar_estruturado(TipoTarefa::TextoLivre, &pedido)
            .await
            .unwrap();

        let recebidos = online.recebidos();
        assert_eq!(recebidos.len(), 2);
        for p in &recebidos {
            let tudo = todo_texto(p).to_lowercase();
            for o in originais {
                assert!(!tudo.contains(&o.to_lowercase()), "vazou um valor sensível");
            }
            assert!(tudo.contains("[aluno_1]"));
            assert!(tudo.contains("[cpf_1]"));
        }
        // A correção (2ª chamada) levou a resposta anterior higienizada.
        assert!(todo_texto(&recebidos[1]).contains("[ALUNO_2]? não sei"));
        assert_eq!(resp.tentativas, 2);
        assert_eq!(resp.valor.intencao, "carta para [ALUNO_1]");
        assert_eq!(
            resp.restaurar().as_str(),
            r#"{"intencao":"carta para Joana Teste Silva"}"#
        );
    }

    #[tokio::test]
    async fn tarefa_sensivel_nunca_chama_motor_online() {
        let online = Arc::new(MotorFalso::novo("online", Alcance::Online).com_respostas([texto("x")]));
        let r = Roteador::novo(hig()).com_motor(online.clone());
        let erro = r
            .gerar_texto(TipoTarefa::Sensivel, &PedidoGeracao::novo("Joana Teste Silva"))
            .await
            .unwrap_err();
        assert!(matches!(erro, ErroIa::SemMotorDisponivel));
        assert_eq!(online.chamadas(), 0);

        // Com local: o local recebe o texto real (a tarefa exige), e o online continua sem chamada.
        let local = Arc::new(MotorFalso::novo("local", Alcance::Local).com_respostas([texto("ok")]));
        let r = Roteador::novo(hig())
            .com_motor(online.clone())
            .com_motor(local.clone());
        let resp = r
            .gerar_texto(TipoTarefa::Sensivel, &PedidoGeracao::novo("Joana Teste Silva"))
            .await
            .unwrap();
        assert_eq!(resp.motor, "local");
        assert_eq!(todo_texto(&local.recebidos()[0]), "Joana Teste Silva");
        assert_eq!(online.chamadas(), 0);
    }

    #[tokio::test]
    async fn estreita_vai_ao_local_primeiro_e_higienizado_por_padrao() {
        let local = Arc::new(MotorFalso::novo("local", Alcance::Local).com_respostas([texto("ok")]));
        let online = Arc::new(MotorFalso::novo("online", Alcance::Online).com_respostas([texto("ok")]));
        let r = Roteador::novo(hig())
            .com_motor(online.clone())
            .com_motor(local.clone());
        let resp = r
            .gerar_texto(TipoTarefa::Estreita, &PedidoGeracao::novo("rótulo de Beto Inventado"))
            .await
            .unwrap();
        assert_eq!(resp.motor, "local");
        assert!(!resp.modo_offline);
        assert_eq!(todo_texto(&local.recebidos()[0]), "rótulo de [ALUNO_1]");
        assert_eq!(online.chamadas(), 0);
    }

    #[tokio::test]
    async fn estreita_sem_local_de_pe_vai_ao_online() {
        let local = Arc::new(
            MotorFalso::novo("local", Alcance::Local)
                .com_respostas([texto("ok")])
                .com_disponivel(false),
        );
        let online = Arc::new(MotorFalso::novo("online", Alcance::Online).com_respostas([texto("ok")]));
        let r = Roteador::novo(hig())
            .com_motor(local.clone())
            .com_motor(online.clone());
        let resp = r
            .gerar_texto(TipoTarefa::Estreita, &PedidoGeracao::novo("x"))
            .await
            .unwrap();
        assert_eq!(resp.motor, "online");
        assert_eq!(local.chamadas(), 0);
        assert_eq!(r.estado().await, EstadoMotores::Online);
    }

    #[tokio::test]
    async fn texto_livre_offline_cai_no_local_com_aviso() {
        let local = Arc::new(MotorFalso::novo("local", Alcance::Local).com_respostas([texto("ok")]));
        let online = Arc::new(MotorFalso::novo("online", Alcance::Online).com_disponivel(false));
        let r = Roteador::novo(hig())
            .com_motor(local.clone())
            .com_motor(online.clone());
        assert_eq!(r.estado().await, EstadoMotores::Local);
        let resp = r
            .gerar_texto(TipoTarefa::TextoLivre, &PedidoGeracao::novo("x"))
            .await
            .unwrap();
        assert_eq!(resp.motor, "local");
        assert!(resp.modo_offline);
        assert_eq!(online.chamadas(), 0);
    }

    #[tokio::test]
    async fn falha_e_timeout_trocam_de_motor_na_hora() {
        let a = Arc::new(MotorFalso::novo("a", Alcance::Online).com_respostas([RespostaFalsa::ErroHttp(500)]));
        let b = Arc::new(MotorFalso::novo("b", Alcance::Online).com_respostas([RespostaFalsa::TempoEsgotado]));
        let c = Arc::new(MotorFalso::novo("c", Alcance::Local).com_respostas([texto(r#"{"intencao":"x"}"#)]));
        let r = Roteador::novo(hig())
            .com_motor(a.clone())
            .com_motor(b.clone())
            .com_motor(c.clone());
        let resp: Resposta<Rotulo> = r
            .gerar_estruturado(TipoTarefa::TextoLivre, &PedidoGeracao::novo("x"))
            .await
            .unwrap();
        assert_eq!(resp.motor, "c");
        assert_eq!((a.chamadas(), b.chamadas(), c.chamadas()), (1, 1, 1));
        assert_eq!(resp.tentativas, 3);
    }

    #[tokio::test]
    async fn fora_do_esquema_duas_vezes_vai_ao_proximo_e_depois_desiste() {
        let a = Arc::new(
            MotorFalso::novo("a", Alcance::Local).com_respostas([texto("nada"), texto("{}"), texto("x")]),
        );
        let b = Arc::new(MotorFalso::novo("b", Alcance::Local).com_respostas([texto(r#"{"intencao":"ok"}"#)]));
        let r = Roteador::novo(hig()).com_motor(a.clone()).com_motor(b.clone());
        let resp: Resposta<Rotulo> = r
            .gerar_estruturado(TipoTarefa::Estreita, &PedidoGeracao::novo("x"))
            .await
            .unwrap();
        assert_eq!(resp.motor, "b");
        assert_eq!(a.chamadas(), 2);
        // A 2ª chamada levou a correção com o erro.
        let segunda = a.recebidos()[1].clone();
        assert_eq!(segunda.mensagens.len(), 3);
        assert!(segunda.mensagens[2].conteudo.contains("esquema JSON"));

        let so_ruim = Arc::new(MotorFalso::novo("a", Alcance::Local).com_respostas([texto("x"), texto("y")]));
        let r = Roteador::novo(hig()).com_motor(so_ruim);
        let erro = r
            .gerar_estruturado::<Rotulo>(TipoTarefa::Estreita, &PedidoGeracao::novo("x"))
            .await
            .unwrap_err();
        assert!(matches!(erro, ErroIa::SaidaForaDoEsquema { tentativas: 2 }));
    }
}
