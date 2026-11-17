//! Painel do aluno: tudo o que ele fez e o que falta nas plataformas, por matéria e por número do registro de
//! aula, com a bandeira de cada item, a nota e a avaliação, os Pause e Responda, as faltas e uma estimativa de
//! média. O professor só manda o link (cifrado, ver `ordoc_crypto::portal`).
//!
//! Regras (pedido do professor em 16/11/2026):
//! - verde: entregou (ou fez o Pause e Responda);
//! - amarela: entregou e a atividade foi liberada para reenviar (veja a avaliação e mande de novo);
//! - vermelha: ainda não fez;
//! - cinza: ainda não liberada.
//!
//! A média é uma ESTIMATIVA a partir das notas das plataformas; a nota oficial é a do professor no Diário.
//! Faltas: atenção a partir de 20% (motor de alertas) e vermelho acima de 25% (frequência mínima de 75%,
//! LDB art. 24, VI).

use serde::{Deserialize, Serialize};

use crate::conector::ocultar_numeros_longos;
use crate::error::{CoreError, Result};

/// Cor do item na tela do aluno.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bandeira {
    Verde,
    Amarela,
    Vermelha,
    Cinza,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoItem {
    /// Tarefa "Registro da Aula (SxxAy)" da plataforma EP.
    RegistroAula,
    /// Atividade H5P "Pause e Responda".
    PauseResponda,
    /// Tarefa do Gerenciador de Atividades (Tarefa SP).
    TarefaSp,
    Outro,
}

/// O que a plataforma diz da entrega.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SituacaoEntrega {
    Entregue,
    ReabertoParaReenvio,
    NaoEntregue,
    NaoLiberada,
}

/// Item de entrada (lido das plataformas; fica só no computador do professor).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemAluno {
    pub materia: String,
    /// "S24A3" (vazio quando não tem).
    #[serde(default)]
    pub codigo: String,
    pub titulo: String,
    pub tipo: TipoItem,
    pub situacao: SituacaoEntrega,
    #[serde(default)]
    pub nota: Option<f64>,
    #[serde(default = "dez")]
    pub nota_maxima: f64,
    /// Comentário do professor (a avaliação em si).
    #[serde(default)]
    pub avaliacao: String,
    /// Pause e Responda: acertou na primeira tentativa?
    #[serde(default)]
    pub primeira_tentativa_certa: Option<bool>,
    #[serde(default)]
    pub tentativas: Option<u32>,
    /// Página da atividade na plataforma (o aluno abre logado).
    #[serde(default)]
    pub link: String,
}

fn dez() -> f64 {
    10.0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaltasMateria {
    pub materia: String,
    pub aulas_dadas: u32,
    pub faltas: u32,
}

/// Entrada de um aluno. `primeiro_nome` só aparece para ele confirmar que o link é dele.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntradaPainel {
    pub primeiro_nome: String,
    pub turma: String,
    /// "16/11/2026 08:30"
    pub atualizado_em: String,
    pub itens: Vec<ItemAluno>,
    #[serde(default)]
    pub faltas: Vec<FaltasMateria>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemPainel {
    pub codigo: String,
    pub titulo: String,
    pub tipo: TipoItem,
    pub bandeira: Bandeira,
    /// "Entregue e avaliado", "Pode reenviar"...
    pub situacao: String,
    /// "8,0 de 10" ou "".
    pub nota: String,
    pub avaliacao: String,
    pub link: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaltasPainel {
    pub aulas_dadas: u32,
    pub faltas: u32,
    pub percentual: f64,
    pub bandeira: Bandeira,
    pub texto: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MateriaPainel {
    pub nome: String,
    pub itens: Vec<ItemPainel>,
    pub verdes: usize,
    pub amarelos: usize,
    pub vermelhos: usize,
    /// Média das atividades já avaliadas (0 a 10).
    pub media_avaliadas: Option<f64>,
    /// Estimativa contando o que falta como pendente (0 até entregar).
    pub media_estimada: Option<f64>,
    pub faltas: Option<FaltasPainel>,
}

fn um() -> u32 {
    1
}

/// Um aluno com o id LOCAL (nunca vai ao site) e o nome completo (só para a lista local de links).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlunoPortal {
    pub id: String,
    pub nome: String,
    pub painel: EntradaPainel,
}

/// Tudo o que o gerador precisa. Fica CIFRADO no computador do professor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntradaPortal {
    /// Versão dos links: aumentar invalida todos os links antigos (rotação).
    #[serde(default = "um")]
    pub versao: u32,
    /// Endereço do site publicado (ex.: `https://usuario.github.io/painel/`). Vazio = só prévia.
    #[serde(default)]
    pub url_base: String,
    pub alunos: Vec<AlunoPortal>,
}

/// Semana do código "S24A3" (None sem código).
pub fn semana_do_codigo(c: &str) -> Option<u32> {
    let (s, _) = chave_codigo(c);
    (s != u32::MAX).then_some(s)
}

/// Atividade de semana depois de `ate_semana` que o aluno ainda não entregou fica cinza ("ainda não liberada"):
/// assim o aluno não vê vermelho em aula que ainda não aconteceu.
pub fn aplicar_semana_limite(itens: &mut [ItemAluno], ate_semana: Option<u32>) {
    let Some(lim) = ate_semana else { return };
    for i in itens {
        if i.situacao == SituacaoEntrega::NaoEntregue && semana_do_codigo(&i.codigo).is_some_and(|s| s > lim) {
            i.situacao = SituacaoEntrega::NaoLiberada;
        }
    }
}

/// O que vai cifrado para o link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PainelAluno {
    pub v: u32,
    pub primeiro_nome: String,
    pub turma: String,
    pub atualizado_em: String,
    pub verdes: usize,
    pub amarelos: usize,
    pub vermelhos: usize,
    pub materias: Vec<MateriaPainel>,
    /// O mais urgente primeiro ("3 registros de aula por fazer em Redes").
    pub avisos: Vec<String>,
}

fn num(x: f64) -> String {
    let t = format!("{:.1}", x);
    t.replace('.', ",")
}

fn texto_limpo(t: &str, campo: &'static str) -> Result<String> {
    let t = t.trim();
    if ocultar_numeros_longos(t) != t {
        return Err(CoreError::DadoPessoalSuspeito(campo));
    }
    Ok(t.to_owned())
}

/// Bandeira e texto de um item.
pub fn classificar(item: &ItemAluno) -> (Bandeira, String) {
    use SituacaoEntrega as S;
    match (item.tipo, item.situacao) {
        (_, S::NaoLiberada) => (Bandeira::Cinza, "Ainda não liberada".into()),
        (_, S::NaoEntregue) => (Bandeira::Vermelha, "Ainda não fez".into()),
        (_, S::ReabertoParaReenvio) => (
            Bandeira::Amarela,
            "Pode reenviar: leia a avaliação, corrija e mande de novo".into(),
        ),
        (TipoItem::PauseResponda, S::Entregue) => {
            let t = match (item.primeira_tentativa_certa, item.tentativas) {
                (Some(true), _) => "Feito: acertou de primeira".to_owned(),
                (Some(false), Some(n)) if n > 1 => format!("Feito: não acertou de primeira ({n} tentativas)"),
                (Some(false), _) => "Feito: não acertou de primeira".to_owned(),
                (None, _) => "Feito".to_owned(),
            };
            (Bandeira::Verde, t)
        }
        (_, S::Entregue) => {
            if item.nota.is_some() {
                (Bandeira::Verde, "Entregue e avaliado".into())
            } else {
                (Bandeira::Verde, "Entregue: aguardando a correção".into())
            }
        }
    }
}

/// Faltas: verde abaixo de 20%, amarela de 20% a 25%, vermelha acima de 25%.
pub fn faltas_painel(f: &FaltasMateria) -> Option<FaltasPainel> {
    if f.aulas_dadas == 0 {
        return None;
    }
    let pct = f64::from(f.faltas) * 100.0 / f64::from(f.aulas_dadas);
    let limite = f64::from(f.aulas_dadas) * 0.25;
    let sobra = (limite - f64::from(f.faltas)).floor();
    let (bandeira, texto) = if pct > 25.0 {
        (
            Bandeira::Vermelha,
            format!(
                "{} faltas em {} aulas ({}%): acima do limite de 25%. Fale com o professor sobre a compensação de ausências.",
                f.faltas,
                f.aulas_dadas,
                num(pct)
            ),
        )
    } else if pct >= 20.0 {
        (
            Bandeira::Amarela,
            format!(
                "{} faltas em {} aulas ({}%): perto do limite de 25%. Evite faltar.",
                f.faltas,
                f.aulas_dadas,
                num(pct)
            ),
        )
    } else {
        (
            Bandeira::Verde,
            format!(
                "{} faltas em {} aulas ({}%). Até agora, mais {} falta(s) ainda ficam abaixo de 25% das aulas dadas.",
                f.faltas,
                f.aulas_dadas,
                num(pct),
                sobra.max(0.0)
            ),
        )
    };
    Some(FaltasPainel { aulas_dadas: f.aulas_dadas, faltas: f.faltas, percentual: pct, bandeira, texto })
}

fn chave_codigo(c: &str) -> (u32, u32) {
    // "S24A3" -> (24, 3); sem código vai para o fim
    let b = c.trim_start_matches(['S', 's']);
    let mut partes = b.split(['A', 'a']);
    let s = partes.next().and_then(|x| x.parse().ok()).unwrap_or(u32::MAX);
    let a = partes.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (s, a)
}

fn ordem_tipo(t: TipoItem) -> u8 {
    match t {
        TipoItem::RegistroAula => 0,
        TipoItem::PauseResponda => 1,
        TipoItem::TarefaSp => 2,
        TipoItem::Outro => 3,
    }
}

/// Monta o painel de um aluno.
pub fn montar_painel(e: &EntradaPainel) -> Result<PainelAluno> {
    let primeiro_nome = texto_limpo(e.primeiro_nome.split_whitespace().next().unwrap_or(""), "painel.nome")?;
    let turma = texto_limpo(&e.turma, "painel.turma")?;
    let mut nomes: Vec<String> = Vec::new();
    for i in &e.itens {
        let m = texto_limpo(&i.materia, "item.materia")?;
        if m.is_empty() {
            return Err(CoreError::IdentificadorAusente("item.materia"));
        }
        if !nomes.contains(&m) {
            nomes.push(m);
        }
    }
    for f in &e.faltas {
        let m = texto_limpo(&f.materia, "faltas.materia")?;
        if !nomes.contains(&m) {
            nomes.push(m);
        }
    }
    nomes.sort();
    let mut materias = Vec::new();
    let mut avisos = Vec::new();
    for nome in nomes {
        let mut doms: Vec<&ItemAluno> = e.itens.iter().filter(|i| i.materia.trim() == nome).collect();
        doms.sort_by_key(|i| (chave_codigo(&i.codigo), ordem_tipo(i.tipo)));
        let mut itens = Vec::new();
        let (mut soma, mut n_aval, mut n_liberados) = (0.0, 0usize, 0usize);
        for i in doms {
            if i.nota_maxima.is_nan() || i.nota_maxima <= 0.0 {
                return Err(CoreError::ForaDoIntervalo {
                    campo: "item.nota_maxima",
                    valor: i.nota_maxima.to_string(),
                    esperado: "> 0",
                });
            }
            if let Some(n) = i.nota {
                if !(0.0..=i.nota_maxima).contains(&n) {
                    return Err(CoreError::ForaDoIntervalo {
                        campo: "item.nota",
                        valor: n.to_string(),
                        esperado: "0 até a nota máxima",
                    });
                }
            }
            let (bandeira, situacao) = classificar(i);
            let conta = matches!(i.tipo, TipoItem::RegistroAula | TipoItem::TarefaSp | TipoItem::Outro);
            if conta && bandeira != Bandeira::Cinza {
                n_liberados += 1;
                if let Some(n) = i.nota {
                    soma += n * 10.0 / i.nota_maxima;
                    n_aval += 1;
                }
            }
            itens.push(ItemPainel {
                codigo: texto_limpo(&i.codigo, "item.codigo")?,
                titulo: texto_limpo(&i.titulo, "item.titulo")?,
                tipo: i.tipo,
                bandeira,
                situacao,
                nota: i.nota.map(|n| format!("{} de {}", num(n), num(i.nota_maxima))).unwrap_or_default(),
                avaliacao: texto_limpo(&i.avaliacao, "item.avaliacao")?,
                link: if i.link.starts_with("https://") { i.link.clone() } else { String::new() },
            });
        }
        let conta_b = |b: Bandeira| itens.iter().filter(|x| x.bandeira == b).count();
        let (verdes, amarelos, vermelhos) = (conta_b(Bandeira::Verde), conta_b(Bandeira::Amarela), conta_b(Bandeira::Vermelha));
        let faltas = e.faltas.iter().find(|f| f.materia.trim() == nome).and_then(faltas_painel);
        if vermelhos > 0 {
            avisos.push(format!("{nome}: {vermelhos} atividade(s) por fazer."));
        }
        if amarelos > 0 {
            avisos.push(format!("{nome}: {amarelos} atividade(s) para reenviar."));
        }
        if let Some(f) = &faltas {
            if f.bandeira != Bandeira::Verde {
                avisos.push(format!("{nome}: {}", f.texto));
            }
        }
        materias.push(MateriaPainel {
            nome,
            itens,
            verdes,
            amarelos,
            vermelhos,
            media_avaliadas: (n_aval > 0).then(|| soma / n_aval as f64),
            media_estimada: (n_liberados > 0).then(|| soma / n_liberados as f64),
            faltas,
        });
    }
    // faltas e reenvio antes de "por fazer" de matéria com muitas pendências? Mantém: faltas no vermelho primeiro.
    avisos.sort_by_key(|a| if a.contains("acima do limite") { 0 } else if a.contains("por fazer") { 1 } else { 2 });
    let soma_b = |f: fn(&MateriaPainel) -> usize| materias.iter().map(f).sum();
    Ok(PainelAluno {
        v: 1,
        primeiro_nome,
        turma,
        atualizado_em: texto_limpo(&e.atualizado_em, "painel.atualizado_em")?,
        verdes: soma_b(|m| m.verdes),
        amarelos: soma_b(|m| m.amarelos),
        vermelhos: soma_b(|m| m.vermelhos),
        materias,
        avisos,
    })
}

#[cfg(test)]
mod testes {
    use super::*;

    fn item(materia: &str, codigo: &str, tipo: TipoItem, situacao: SituacaoEntrega, nota: Option<f64>) -> ItemAluno {
        ItemAluno {
            materia: materia.into(),
            codigo: codigo.into(),
            titulo: format!("Registro da Aula ({codigo})"),
            tipo,
            situacao,
            nota,
            nota_maxima: 10.0,
            avaliacao: String::new(),
            primeira_tentativa_certa: None,
            tentativas: None,
            link: String::new(),
        }
    }

    #[test]
    fn bandeiras_seguem_o_pedido() {
        use SituacaoEntrega as S;
        let r = |s| classificar(&item("Redes", "S24A3", TipoItem::RegistroAula, s, None)).0;
        assert_eq!(r(S::Entregue), Bandeira::Verde);
        assert_eq!(r(S::ReabertoParaReenvio), Bandeira::Amarela);
        assert_eq!(r(S::NaoEntregue), Bandeira::Vermelha);
        assert_eq!(r(S::NaoLiberada), Bandeira::Cinza);
        let mut p = item("Redes", "S24A1", TipoItem::PauseResponda, S::Entregue, Some(10.0));
        p.primeira_tentativa_certa = Some(false);
        p.tentativas = Some(3);
        assert_eq!(classificar(&p).1, "Feito: não acertou de primeira (3 tentativas)");
    }

    #[test]
    fn medias_e_ordem_por_semana() {
        use SituacaoEntrega as S;
        let e = EntradaPainel {
            primeiro_nome: "Ana Maria".into(),
            turma: "2ª A".into(),
            atualizado_em: "16/11/2026 08:00".into(),
            itens: vec![
                item("Redes", "S25A1", TipoItem::RegistroAula, S::NaoEntregue, None),
                item("Redes", "S24A3", TipoItem::RegistroAula, S::Entregue, Some(8.0)),
                item("Redes", "S23A3", TipoItem::RegistroAula, S::ReabertoParaReenvio, Some(1.0)),
                item("Redes", "S24A2", TipoItem::PauseResponda, S::Entregue, Some(10.0)),
                item("Redes", "S26A3", TipoItem::RegistroAula, S::NaoLiberada, None),
            ],
            faltas: vec![FaltasMateria { materia: "Redes".into(), aulas_dadas: 40, faltas: 11 }],
        };
        let p = montar_painel(&e).unwrap();
        assert_eq!(p.primeiro_nome, "Ana");
        let m = &p.materias[0];
        let codigos: Vec<&str> = m.itens.iter().map(|i| i.codigo.as_str()).collect();
        assert_eq!(codigos, ["S23A3", "S24A2", "S24A3", "S25A1", "S26A3"]);
        // registros liberados: S23A3 (1), S24A3 (8), S25A1 (sem nota) -> avaliadas 4,5; estimada 3,0
        assert!((m.media_avaliadas.unwrap() - 4.5).abs() < 1e-9);
        assert!((m.media_estimada.unwrap() - 3.0).abs() < 1e-9);
        assert_eq!((p.verdes, p.amarelos, p.vermelhos), (2, 1, 1));
        let f = m.faltas.as_ref().unwrap();
        assert_eq!(f.bandeira, Bandeira::Vermelha); // 27,5%
        assert!(p.avisos[0].contains("acima do limite"));
    }

    #[test]
    fn semana_limite_deixa_cinza_so_o_futuro_nao_entregue() {
        use SituacaoEntrega as S;
        let mut v = vec![
            item("R", "S25A1", TipoItem::RegistroAula, S::NaoEntregue, None),
            item("R", "S27A2", TipoItem::RegistroAula, S::NaoEntregue, None),
            item("R", "S28A1", TipoItem::RegistroAula, S::Entregue, Some(9.0)),
        ];
        aplicar_semana_limite(&mut v, Some(26));
        assert_eq!(v[0].situacao, S::NaoEntregue);
        assert_eq!(v[1].situacao, S::NaoLiberada);
        assert_eq!(v[2].situacao, S::Entregue);
    }

    #[test]
    fn faltas_limites() {
        let f = |faltas| faltas_painel(&FaltasMateria { materia: "X".into(), aulas_dadas: 40, faltas }).unwrap().bandeira;
        assert_eq!(f(7), Bandeira::Verde);
        assert_eq!(f(8), Bandeira::Amarela); // 20%
        assert_eq!(f(10), Bandeira::Amarela); // 25%
        assert_eq!(f(11), Bandeira::Vermelha);
        assert!(faltas_painel(&FaltasMateria { materia: "X".into(), aulas_dadas: 0, faltas: 0 }).is_none());
    }

    #[test]
    fn recusa_numero_longo_e_nota_fora() {
        use SituacaoEntrega as S;
        let mut e = EntradaPainel {
            primeiro_nome: "Bia".into(),
            turma: "3ª B".into(),
            atualizado_em: "hoje".into(),
            itens: vec![item("BD", "S24A2", TipoItem::RegistroAula, S::Entregue, Some(11.0))],
            faltas: vec![],
        };
        assert!(matches!(montar_painel(&e), Err(CoreError::ForaDoIntervalo { .. })));
        e.itens[0].nota = Some(9.0);
        e.itens[0].avaliacao = "RA 123456789012".into();
        assert_eq!(montar_painel(&e), Err(CoreError::DadoPessoalSuspeito("item.avaliacao")));
    }
}
