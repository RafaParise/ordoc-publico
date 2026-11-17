//! ordoc-core: o domínio do Ordoc.
//!
//! - `modelos`: modelos canônicos unificados (Turma, Aluno, Aula, RegistroFrequencia, AtividadeSdf,
//!   IndicadorBi, IndicadorRisco e os auxiliares Matricula e EntregaSdf). Todo modelo que vem de uma
//!   plataforma guarda o JSON bruto em `metadata_bruta`; o IndicadorRisco, calculado aqui, guarda
//!   `evidencias`.
//! - `conector`: a trait assíncrona `IEducationalConnector` (Padrão Adapter), a versão dinâmica
//!   `ConectorDinamico` e a telemetria de conectores.
//! - `alert_rules`: o motor local de alertas.
//! - `presenca_professor`: o que falta no Diário para a aula ficar verde no Professor Presente e
//!   até quando cada perfil ainda lança (caso de uso principal).
//! - `situacao_funcional`: o painel do que falta para o professor ficar 100% regularizado
//!   (pendências de vários sistemas, prioridade, bloqueios e percentual). Só descreve ações.
//! - `visao`: o contrato com a interface (dados prontos para a tela; compila em WASM).
//!
//! Domínio puro: sem rede, sem banco e sem os parsers. Não depende de nenhum outro crate do Ordoc.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod alert_rules;
pub mod conector;
pub mod desempenho;
pub mod error;
pub mod modelos;
pub mod portal_aluno;
pub mod presenca_professor;
pub mod situacao_funcional;
pub mod visao;

pub use alert_rules::{
    LimitesAlerta, MotorAlertas, RegraAlerta, RegraFaltasSemEntrega, RegraFrequenciaBaixa,
    RegraSemEntregas, ResumoAluno, REGRA_FALTAS_SEM_ENTREGA, REGRA_FREQUENCIA_BAIXA,
    REGRA_SEM_ENTREGAS,
};
pub use conector::{
    coletar_com_telemetria, enviar_com_telemetria, sanitizar_detalhe, AcaoAposErro,
    ConectorDinamico, ErroConector, FuturoConector, IEducationalConnector, LoteCanonico,
    OperacaoRemota, Plataforma, ResultadoEnvio, StatusConector, TelemetriaConector, LIMITE_DETALHE,
};
pub use desempenho::{
    classificar_resposta, discrepancia_turma, discrepancia_turma_com_limites, proximo_passo,
    risco_final, sintese_bimestral, situacao_frequencia, AlertaDesempenho, AlertaTurma,
    Arredondamento, DetalheInstrumento, DirecaoDiscrepancia, InstrumentoAvaliacao, LimitesTurma,
    PoliticaRecuperacao, RegrasAvaliacao, RegrasFrequencia, RespostaPedido, RiscoFinal,
    RiscoFrequencia, SinteseBimestral, SituacaoFrequencia, TipoInstrumento, NOTA_MAXIMA,
    NOTA_SATISFATORIA,
};
pub use error::{CoreError, Result};
pub use modelos::{
    validar_data_iso, Aluno, AtividadeSdf, Aula, EntregaSdf, IndicadorBi, IndicadorRisco,
    Matricula, RegistroFrequencia, Severidade, StatusPresenca, StatusSincronizacao, Turma,
    RA_AUSENTE,
};
pub use presenca_professor::{
    cor_do_dia, origem_da_chamada, pendencias, AulaDiario, CorDiaProfessor, EstadoAulaProfessor,
    JanelasEdicao, LancamentoDiario, OrigemChamada, PendenciaPresenca, QuemPodeLancar,
};
pub use situacao_funcional::{
    gerar_id, Acao, AreaPendencia, Bloqueio, EstadoPendencia, ItemAgenda, PainelSituacao,
    Pendencia, Prioridade, Progresso, Protocolo, ResumoRegularizacao, SistemaOrigem,
    LIMIAR_PRAZO_VENCENDO_DIAS,
};

/// Agora, em milissegundos desde 1970 (UTC). Um relógio antes de 1970 vira 0.
pub fn agora_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod testes {
    #[test]
    fn relogio_em_milissegundos() {
        // 2026-01-01T00:00:00Z = 1_767_225_600_000 ms.
        assert!(super::agora_ms() > 1_767_225_600_000);
    }
}
