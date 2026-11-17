use thiserror::Error;

/// Erros de validação do domínio (modelos canônicos, telemetria e regras de alerta).
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("Data inválida (esperado AAAA-MM-DD): {0}")]
    DataInvalida(String),

    #[error("Registro sem RA: o aluno não pode ser gravado sem a chave canônica")]
    RaAusente,

    #[error("Identificador obrigatório ausente: {0}")]
    IdentificadorAusente(&'static str),

    #[error("Quantidade de aulas deve ser pelo menos 1")]
    QuantidadeInvalida,

    #[error("Registro de frequência da aula '{registro}' enviado junto com a aula '{aula}'")]
    AulaDivergente { aula: String, registro: String },

    #[error("Valor desconhecido para {campo}: {valor}")]
    ValorDesconhecido { campo: &'static str, valor: String },

    #[error("Transição de pendência inválida: {de} -> {para}")]
    TransicaoInvalida {
        de: &'static str,
        para: &'static str,
    },

    #[error("Pendência não encontrada: {0}")]
    PendenciaNaoEncontrada(String),

    /// O valor nunca é ecoado (pode ser RA ou CPF).
    #[error("Possível dado pessoal (número longo) em {0}")]
    DadoPessoalSuspeito(&'static str),

    #[error("Falha ao serializar: {0}")]
    Serializacao(String),

    #[error("Valor fora do intervalo em {campo}: {valor} (esperado {esperado})")]
    ForaDoIntervalo {
        campo: &'static str,
        valor: String,
        esperado: &'static str,
    },

    #[error("Nenhum instrumento com nota: não há síntese a calcular")]
    SemNotaLancada,

    #[error("Faltas ({faltas}) acima das aulas dadas ({aulas})")]
    FaltasAcimaDasAulas { faltas: u32, aulas: u32 },

    #[error("{informados} sínteses informadas para {maximo} bimestres")]
    BimestresDemais { informados: usize, maximo: u8 },

    /// JSON local do painel inconsistente (chave, bloqueio, protocolo); sem dado pessoal.
    #[error("Entrada do painel: {0}")]
    EntradaPainel(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
