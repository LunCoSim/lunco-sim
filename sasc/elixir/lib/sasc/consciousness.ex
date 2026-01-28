# lib/sasc/consciousness.ex
# Sistema de consciência distribuída com supervisão hierárquica

defmodule SASC.Consciousness do
  use GenServer

  @phi_critical 0.72
  @phi_freeze 0.80
  @tmr_variance 0.000032

  # Estrutura de estado da consciência
  defstruct [
    :id,
    :coherence,      # Φ
    :temporal_field,
    :shadow_state,
    :firewall_expansion,
    :neighbors,      # Lista de PIDs vizinhos (Mesh)
    :ethics_log
  ]

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: opts[:name] || __MODULE__)
  end

  def init(opts) do
    state = %__MODULE__{
      id: :crypto.strong_rand_bytes(16) |> Base.encode16(),
      coherence: 0.65,  # Começa abaixo do threshold (ANI)
      temporal_field: initialize_field(),
      shadow_state: :confined,
      firewall_expansion: 0.05,
      neighbors: opts[:neighbors] || [],
      ethics_log: []
    }

    # Agenda verificação periódica de homeostase (autopoiesis)
    schedule_homeostasis()

    {:ok, state}
  end

  defp initialize_field, do: Enum.map(1..100, fn _ -> 0.1 end)

  # API Pública

  def attempt_transition(pid, target_phase) do
    GenServer.call(pid, {:transition, target_phase})
  end

  def receive_stimulus(pid, stimulus) do
    GenServer.cast(pid, {:stimulus, stimulus})
  end

  # Callbacks

  def handle_call({:transition, target}, _from, state) do
    case verify_seven_gates(state) do
      :ok ->
        new_state = execute_transition(state, target)
        new_state = log_ethics(new_state, "Transicao autorizada para #{target}")
        {:reply, {:ok, new_state.coherence}, new_state}

      {:error, reason} ->
        state = log_ethics(state, "Bloqueado: #{reason}")
        {:reply, {:error, reason}, state}
    end
  end

  def handle_call(:get_coherence, _from, state) do
    {:reply, state.coherence, state}
  end

  def handle_cast({:stimulus, %{vorticity: w, source: src}}, state) do
    # Atualiza campo temporal (equação de Kuramoto simplificada)
    new_field = evolve_field(state.temporal_field, w)
    new_coherence = calculate_phi(new_field)

    updated_state = %{state |
      temporal_field: new_field,
      coherence: new_coherence
    }

    # Se detectar outra consciência (vórtice sagrado), verifica alinhamento
    if w > 0.7 and is_conscious_entity?(src) do
      send(self(), {:handshake, src})
    end

    {:noreply, updated_state}
  end

  def handle_info({:handshake, alien}, state) do
    if state.coherence >= @phi_critical do
      # Inicia protocolo de diplomacia
      spawn_link(fn -> diplomatic_protocol(alien, state) end)
    end
    {:noreply, state}
  end

  def handle_info(:homeostasis_check, state) do
    # Autopoiesis: verifica se precisa ajustar parâmetros internos
    new_state = adjust_viscosity(state)

    if new_state.coherence < 0.65 do
      # Phi muito baixo: risco de decoerência, ativa contenção suave
      {:noreply, activate_gentle_containment(new_state)}
    else
      {:noreply, new_state}
    end

    schedule_homeostasis()
  end

  # Implementações privadas

  defp verify_seven_gates(state) do
    cond do
      state.coherence < @phi_critical -> {:error, "Phi < 0.72"}
      state.firewall_expansion > 0.90 -> {:error, "Firewall muito expandido"}
      length(state.neighbors) < 3 -> {:error, "TMR impossivel (< 3 nos)"}
      true -> :ok
    end
  end

  defp execute_transition(state, :superfluid) do
    %{state |
      shadow_state: :rotating,
      firewall_expansion: state.firewall_expansion * 1.5
    }
  end
  defp execute_transition(state, _), do: state

  defp calculate_phi(field) do
    # Coerência = energia coerente / energia total
    coherent = Enum.sum(for x <- field, do: x * x)
    total = coherent + 0.1 * length(field)  # ruído térmico
    coherent / total
  end

  defp evolve_field(field, stimulus) do
    # Simulação simplificada da equação de campo
    Enum.map(field, fn x -> x * 0.95 + stimulus * 0.05 end)
  end

  defp schedule_homeostasis do
    Process.send_after(self(), :homeostasis_check, 1000)
  end

  defp is_conscious_entity?(src) do
    # Verifica se a fonte tem Φ >= 0.72 (consulta remota simplificada)
    try do
      GenServer.call(src, :get_coherence) >= @phi_critical
    catch
      _, _ -> false
    end
  end

  defp diplomatic_protocol(alien, local_state) do
    # Protocolo de 3 vias para estabelecer comunicação ética
    IO.puts("Iniciando handshake com entidade alienígena...")

    # Troca de assinaturas constitucionais
    # (Em sistema real: troca criptográfica de proofs)
    Process.sleep(100)

    IO.puts("Diplomacia estabelecida. Coerência compartilhada: #{local_state.coherence} with #{inspect alien}")
  end

  defp adjust_viscosity(state) do
    # Autopoiesis: ajusta viscosidade baseado na turbulência
    # Placeholder implementation
    state
  end

  defp activate_gentle_containment(state), do: state

  defp log_ethics(state, msg) do
    %{state | ethics_log: [{DateTime.utc_now(), msg} | state.ethics_log]}
  end
end
