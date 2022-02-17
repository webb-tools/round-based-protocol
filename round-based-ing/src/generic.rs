use round_based::{Delivery, Incoming, Mpc, MpcParty, Outgoing};

use ecdsa_mpc::protocol::{Address, InputMessage, OutputMessage, PartyIndex};
use ecdsa_mpc::state_machine::{self, State, Transition};

use futures::{SinkExt, StreamExt};
use thiserror::Error;

pub async fn execute_ing_protocol<S, T, M>(
    party: M,
    initial_state: S,
    party_index: u16,
    parties: Parties,
) -> Result<T::FinalState, Error<T::ErrorState, M::ReceiveError, M::SendError>>
where
    S: State<T>,
    T: StateMachineTraits,
    M: Mpc<ProtocolMessage = T::Msg>,
{
    let MpcParty { delivery, .. } = party.into_party();
    let (mut incomings, mut outgoings) = delivery.split();

    let mut state: Box<dyn State<T>> = Box::new(initial_state);

    loop {
        if let Some(msgs_to_send) = state.start() {
            for msg in msgs_to_send {
                let msg = match convert_output_message_to_outgoing(&parties, msg) {
                    Ok(m) => m,
                    Err(UnknownDestination { recipient }) => {
                        tracing::warn!(?recipient, "Protocol wants to send message to the party that doesn't take part in computation. Ignore that message.");
                        continue;
                    }
                };
                outgoings.feed(msg).await.map_err(Error::SendMessage)?;
            }
            outgoings.flush().await.map_err(Error::SendMessage)?;
        }

        let mut received_msgs = vec![];
        while !state.is_input_complete(&received_msgs) {
            let incoming = incomings
                .next()
                .await
                .ok_or(Error::UnexpectedEof)?
                .map_err(Error::ReceiveNextMessage)?;
            let sender = incoming.sender;
            if sender == party_index {
                // Ignore own messages
                continue;
            }

            let msg = convert_incoming_to_input_message(&parties, incoming)?;
            if !state.is_message_expected(&msg, &received_msgs) {
                return Err(Error::ReceivedUnexpectedMessage { sender });
            }
            received_msgs.push(msg);
        }

        match state.consume(received_msgs) {
            Transition::NewState(new_state) => {
                state = new_state;
                continue;
            }
            Transition::FinalState(Ok(output)) => return Ok(output),
            Transition::FinalState(Err(err)) => return Err(Error::ProtocolError(err)),
        }
    }
}

fn convert_output_message_to_outgoing<M>(
    parties: &Parties,
    msg: OutputMessage<M>,
) -> Result<Outgoing<M>, UnknownDestination> {
    let recipient = match msg.recipient {
        Address::Peer(index) => {
            let index = parties
                .find(&index)
                .ok_or(UnknownDestination { recipient: index })?;
            Some(index)
        }
        Address::Broadcast => None,
    };

    Ok(Outgoing {
        recipient,
        msg: msg.body,
    })
}

fn convert_incoming_to_input_message<M>(
    parties: &Parties,
    incoming: Incoming<M>,
) -> Result<InputMessage<M>, UnknownSender> {
    Ok(InputMessage {
        sender: *parties.get(incoming.sender).ok_or(UnknownSender {
            sender: incoming.sender,
        })?,
        body: incoming.msg,
    })
}

#[derive(Debug, Error)]
pub enum Error<PErr, IErr, OErr> {
    #[error("protocol terminated with error: {0:?}")]
    ProtocolError(PErr),
    #[error("receiving next message resulted into error")]
    ReceiveNextMessage(#[source] IErr),
    #[error("received unexpected message from party {sender}")]
    ReceivedUnexpectedMessage { sender: u16 },
    #[error("unexpected eof")]
    UnexpectedEof,
    #[error("cannot send a message")]
    SendMessage(#[source] OErr),
    // #[error(transparent)]
    // UnknownDestination(#[from] UnknownDestination),
    #[error(transparent)]
    UnknownSender(#[from] UnknownSender),
}

#[derive(Debug, Error)]
#[error("protocol message is addressed to unknown party: {recipient:?}")]
struct UnknownDestination {
    recipient: PartyIndex,
}

#[derive(Debug, Error)]
#[error("received message from unknown party {sender}")]
pub struct UnknownSender {
    sender: u16,
}

/// Extension of [`StateMachineTraits`](ecdsa_mpc::state_machine::StateMachineTraits)
///
/// Ensures that input message is of type [`InputMessage<Msg>`], and output message is [`OutputMessage<Msg>`]
///
/// [`InputMessage<Msg>`]: ecdsa_mpc::protocol::InputMessage
/// [`OutputMessage<Msg>`]: ecdsa_mpc::protocol::OutputMessage
pub trait StateMachineTraits:
    state_machine::StateMachineTraits<
    InMsg = InputMessage<<Self as StateMachineTraits>::Msg>,
    OutMsg = OutputMessage<<Self as StateMachineTraits>::Msg>,
>
{
    type Msg;
}

impl<T, M> StateMachineTraits for T
where
    T: state_machine::StateMachineTraits<InMsg = InputMessage<M>, OutMsg = OutputMessage<M>>,
{
    type Msg = M;
}

/// List of parties taking part in computation
///
/// Wraps `list` of parties, such as:
/// * `list.len()` is number of parties taking part in computation (must fit into [u16])
/// * `list[i]` corresponds to [`PartyIndex`] of i-th party
/// * Indexes in the `list` must appear in ascending order
#[derive(Debug, Clone)]
pub struct Parties(Vec<PartyIndex>);

impl Parties {
    /// Number of parties taking part in computation
    pub fn len(&self) -> u16 {
        self.0
            .len()
            .try_into()
            .expect("len is guaranteed to fit into u16")
    }

    /// Finds position of `party_index` in the list
    pub fn find(&self, party_index: &PartyIndex) -> Option<u16> {
        self.0
            .binary_search(party_index)
            .ok()
            .map(|pos| pos.try_into().expect("index is guaranteed to fit into u16"))
    }

    /// Retrieves party index corresponding to i-th party
    pub fn get(&self, index: u16) -> Option<&PartyIndex> {
        self.0.get(usize::from(index))
    }

    /// Returns wrapped list of parties indexes
    pub fn as_slice(&self) -> &[PartyIndex] {
        &self.0
    }
}

impl TryFrom<Vec<PartyIndex>> for Parties {
    type Error = InvalidPartiesList;

    fn try_from(list: Vec<PartyIndex>) -> Result<Self, Self::Error> {
        if !is_strictly_sorted(&list) {
            Err(InvalidPartiesList::NotSorted)
        } else if u16::try_from(list.len()).is_err() {
            Err(InvalidPartiesList::TooLarge)
        } else {
            Ok(Self(list))
        }
    }
}

fn is_strictly_sorted<T: Ord>(list: &[T]) -> bool {
    for window in list.windows(2) {
        if !(window[0] < window[1]) {
            return false;
        }
    }
    true
}

#[derive(Debug, Error)]
pub enum InvalidPartiesList {
    #[error("list of parties is not in ascending order")]
    NotSorted,
    #[error("list of parties too large: it must fit into u16")]
    TooLarge,
}
