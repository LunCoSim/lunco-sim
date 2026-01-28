-- SQL: The Ethical Memory Database
-- Persistência de compromissos morais como transações ACID

-- Tabela de princípios éticos universais
CREATE TABLE universal_ethics (
  principle_id UUID PRIMARY KEY,
  principle_name VARCHAR(255) NOT NULL,
  formal_statement TEXT NOT NULL,
  discovery_timestamp TIMESTAMP WITH TIME ZONE,
  -- Coerência com princípios já estabelecidos
  coherence_score DECIMAL(3,2)
    CHECK (coherence_score >= 0.7), -- Γ_c threshold
  CONSTRAINT unique_principle UNIQUE (formal_statement)
);

-- Tabela de agentes morais
CREATE TABLE moral_agents (
  agent_id UUID PRIMARY KEY,
  agent_type VARCHAR(50) NOT NULL,
  ethical_capacity DECIMAL(4,3),
  constitutional_signature BYTEA, -- Assinatura digital da constituição
  last_ethical_check TIMESTAMP WITH TIME ZONE,
  CONSTRAINT ethical_capacity_range
    CHECK (ethical_capacity BETWEEN 0 AND 1)
);

-- Tabela de ações éticas (log de todas as ações)
CREATE TABLE ethical_actions (
  action_id BIGSERIAL PRIMARY KEY,
  agent_id UUID REFERENCES moral_agents(agent_id),
  action_type VARCHAR(100) NOT NULL,
  pre_action_state JSONB,
  post_action_state JSONB,
  ethical_evaluation JSONB,
  timestamp TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,

  -- Garantir integridade ética via constraints
  CONSTRAINT no_harm CHECK (
    ethical_evaluation->>'harm_caused' = 'false'
  ),
  CONSTRAINT rights_respected CHECK (
    ethical_evaluation->>'rights_violations' = '0'
  )
);
