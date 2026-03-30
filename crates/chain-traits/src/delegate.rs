/// Delegates all chain trait impls from a newtype to its inner type.
///
/// ```ignore
/// // with generics:
/// mercury_chain_traits::delegate_chain! {
///     impl[S: CosmosSigner] CosmosAdapter<S> => CosmosChain<S>
/// }
///
/// // without generics:
/// mercury_chain_traits::delegate_chain! {
///     impl[] EthereumAdapter => EthereumChain
/// }
///
/// // skip blanket ClientPayloadBuilder (for custom cross-chain impls):
/// mercury_chain_traits::delegate_chain! {
///     impl[] EthereumAdapter => EthereumChain; skip_cpb
/// }
/// ```
#[macro_export]
macro_rules! delegate_chain {
    (impl[$($gen:tt)*] $Wrapper:ty => $Inner:ty; skip_cpb) => {
        $crate::delegate_chain!(@base [$($gen)*] $Wrapper, $Inner);
    };
    (impl[$($gen:tt)*] $Wrapper:ty => $Inner:ty) => {
        $crate::delegate_chain!(@base [$($gen)*] $Wrapper, $Inner);
        $crate::delegate_chain!(@cpb [$($gen)*] $Wrapper, $Inner);
    };
    (@base [$($gen:tt)*] $Wrapper:ty, $Inner:ty) => {
        impl<$($gen)*> ::std::ops::Deref for $Wrapper {
            type Target = $Inner;
            #[inline]
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl<$($gen)*> $crate::inner::HasCore for $Wrapper {
            type Core = $Inner;
        }

        impl<$($gen)*> $crate::ChainTypes for $Wrapper {
            type Height = <$Inner as $crate::ChainTypes>::Height;
            type Timestamp = <$Inner as $crate::ChainTypes>::Timestamp;
            type ChainId = <$Inner as $crate::ChainTypes>::ChainId;
            type ClientId = <$Inner as $crate::ChainTypes>::ClientId;
            type Event = <$Inner as $crate::ChainTypes>::Event;
            type Message = <$Inner as $crate::ChainTypes>::Message;
            type MessageResponse = <$Inner as $crate::ChainTypes>::MessageResponse;
            type ChainStatus = <$Inner as $crate::ChainTypes>::ChainStatus;

            fn chain_status_height(status: &Self::ChainStatus) -> &Self::Height {
                <$Inner as $crate::ChainTypes>::chain_status_height(status)
            }
            fn chain_status_timestamp(status: &Self::ChainStatus) -> &Self::Timestamp {
                <$Inner as $crate::ChainTypes>::chain_status_timestamp(status)
            }
            fn chain_status_timestamp_secs(status: &Self::ChainStatus) -> u64 {
                <$Inner as $crate::ChainTypes>::chain_status_timestamp_secs(status)
            }
            fn revision_number(&self) -> u64 {
                self.0.revision_number()
            }
            fn increment_height(height: &Self::Height) -> Option<Self::Height> {
                <$Inner as $crate::ChainTypes>::increment_height(height)
            }
            fn sub_height(height: &Self::Height, n: u64) -> Option<Self::Height> {
                <$Inner as $crate::ChainTypes>::sub_height(height, n)
            }
            fn block_time(&self) -> ::std::time::Duration {
                self.0.block_time()
            }
            fn max_clock_drift(&self) -> ::std::time::Duration {
                self.0.max_clock_drift()
            }
            fn chain_id(&self) -> &Self::ChainId {
                self.0.chain_id()
            }
            fn chain_label(&self) -> $crate::_mercury_core::ChainLabel {
                self.0.chain_label()
            }
        }

        impl<$($gen)*> $crate::IbcTypes for $Wrapper {
            type ClientState = <$Inner as $crate::IbcTypes>::ClientState;
            type ConsensusState = <$Inner as $crate::IbcTypes>::ConsensusState;
            type CommitmentProof = <$Inner as $crate::IbcTypes>::CommitmentProof;
            type Packet = <$Inner as $crate::IbcTypes>::Packet;
            type PacketCommitment = <$Inner as $crate::IbcTypes>::PacketCommitment;
            type PacketReceipt = <$Inner as $crate::IbcTypes>::PacketReceipt;
            type Acknowledgement = <$Inner as $crate::IbcTypes>::Acknowledgement;

            fn packet_sequence(packet: &Self::Packet) -> $crate::types::PacketSequence {
                <$Inner as $crate::IbcTypes>::packet_sequence(packet)
            }
            fn packet_timeout_timestamp(packet: &Self::Packet) -> $crate::types::TimeoutTimestamp {
                <$Inner as $crate::IbcTypes>::packet_timeout_timestamp(packet)
            }
            fn packet_source_ports(packet: &Self::Packet) -> Vec<$crate::types::Port> {
                <$Inner as $crate::IbcTypes>::packet_source_ports(packet)
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::MessageSender for $Wrapper {
            async fn send_messages(
                &self,
                messages: Vec<Self::Message>,
            ) -> $crate::_mercury_core::error::Result<$crate::TxReceipt> {
                self.0.send_messages(messages).await
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::queries::ChainStatusQuery for $Wrapper {
            async fn query_chain_status(
                &self,
            ) -> $crate::_mercury_core::error::Result<Self::ChainStatus> {
                self.0.query_chain_status().await
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::queries::PacketStateQuery for $Wrapper {
            async fn query_packet_commitment(
                &self,
                client_id: &Self::ClientId,
                sequence: $crate::types::PacketSequence,
                height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<(
                Option<Self::PacketCommitment>,
                Self::CommitmentProof,
            )> {
                self.0
                    .query_packet_commitment(client_id, sequence, height)
                    .await
            }

            async fn query_packet_receipt(
                &self,
                client_id: &Self::ClientId,
                sequence: $crate::types::PacketSequence,
                height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<(
                Option<Self::PacketReceipt>,
                Self::CommitmentProof,
            )> {
                self.0
                    .query_packet_receipt(client_id, sequence, height)
                    .await
            }

            async fn query_packet_acknowledgement(
                &self,
                client_id: &Self::ClientId,
                sequence: $crate::types::PacketSequence,
                height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<(
                Option<Self::Acknowledgement>,
                Self::CommitmentProof,
            )> {
                self.0
                    .query_packet_acknowledgement(client_id, sequence, height)
                    .await
            }

            async fn query_commitment_sequences(
                &self,
                client_id: &Self::ClientId,
                height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<Vec<$crate::types::PacketSequence>> {
                self.0.query_commitment_sequences(client_id, height).await
            }

            async fn query_ack_sequences(
                &self,
                client_id: &Self::ClientId,
                height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<Vec<$crate::types::PacketSequence>> {
                self.0.query_ack_sequences(client_id, height).await
            }

            fn commitment_to_membership_entry(
                &self,
                client_id: &Self::ClientId,
                sequence: $crate::types::PacketSequence,
                commitment: &Self::PacketCommitment,
                proof: &Self::CommitmentProof,
            ) -> Option<$crate::_mercury_core::MembershipProofEntry> {
                self.0
                    .commitment_to_membership_entry(client_id, sequence, commitment, proof)
            }

            fn ack_to_membership_entry(
                &self,
                client_id: &Self::ClientId,
                sequence: $crate::types::PacketSequence,
                ack: &Self::Acknowledgement,
                proof: &Self::CommitmentProof,
            ) -> Option<$crate::_mercury_core::MembershipProofEntry> {
                self.0
                    .ack_to_membership_entry(client_id, sequence, ack, proof)
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::events::PacketEvents for $Wrapper {
            type SendPacketEvent = <$Inner as $crate::events::PacketEvents>::SendPacketEvent;
            type WriteAckEvent = <$Inner as $crate::events::PacketEvents>::WriteAckEvent;

            fn try_extract_send_packet_event(
                event: &Self::Event,
            ) -> Option<Self::SendPacketEvent> {
                <$Inner as $crate::events::PacketEvents>::try_extract_send_packet_event(event)
            }
            fn try_extract_write_ack_event(
                event: &Self::Event,
            ) -> Option<Self::WriteAckEvent> {
                <$Inner as $crate::events::PacketEvents>::try_extract_write_ack_event(event)
            }
            fn packet_from_send_event(event: &Self::SendPacketEvent) -> &Self::Packet {
                <$Inner as $crate::events::PacketEvents>::packet_from_send_event(event)
            }
            fn packet_from_write_ack_event(
                event: &Self::WriteAckEvent,
            ) -> (&Self::Packet, &Self::Acknowledgement) {
                <$Inner as $crate::events::PacketEvents>::packet_from_write_ack_event(event)
            }
            async fn query_block_events(
                &self,
                height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<Vec<Self::Event>> {
                self.0.query_block_events(height).await
            }
            async fn query_send_packet_event(
                &self,
                client_id: &Self::ClientId,
                sequence: $crate::types::PacketSequence,
            ) -> $crate::_mercury_core::error::Result<Option<Self::SendPacketEvent>> {
                self.0.query_send_packet_event(client_id, sequence).await
            }

            async fn query_write_ack_event(
                &self,
                client_id: &Self::ClientId,
                sequence: $crate::types::PacketSequence,
            ) -> $crate::_mercury_core::error::Result<Option<Self::WriteAckEvent>> {
                self.0.query_write_ack_event(client_id, sequence).await
            }

            async fn subscribe_block_events(
                &self,
            ) -> $crate::_mercury_core::error::Result<
                Option<$crate::events::BlockEventStream<Self::Height, Self::Event>>,
            > {
                self.0.subscribe_block_events().await
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::queries::ClientQuery<$Inner> for $Wrapper
        where
            $Inner: $crate::queries::ClientQuery<$Inner>,
        {
            async fn query_client_state(
                &self,
                client_id: &Self::ClientId,
                height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<Self::ClientState> {
                self.0.query_client_state(client_id, height).await
            }

            async fn query_consensus_state(
                &self,
                client_id: &Self::ClientId,
                consensus_height: &Self::Height,
                query_height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<Self::ConsensusState> {
                self.0
                    .query_consensus_state(client_id, consensus_height, query_height)
                    .await
            }

            fn trusting_period(
                client_state: &Self::ClientState,
            ) -> Option<::std::time::Duration> {
                <$Inner as $crate::queries::ClientQuery<$Inner>>::trusting_period(client_state)
            }

            fn client_latest_height(client_state: &Self::ClientState) -> Self::Height {
                <$Inner as $crate::queries::ClientQuery<$Inner>>::client_latest_height(
                    client_state,
                )
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::builders::ClientMessageBuilder<$Inner> for $Wrapper
        where
            $Inner: $crate::builders::ClientMessageBuilder<$Inner>,
        {
            type CreateClientPayload =
                <$Inner as $crate::builders::ClientMessageBuilder<$Inner>>::CreateClientPayload;
            type UpdateClientPayload =
                <$Inner as $crate::builders::ClientMessageBuilder<$Inner>>::UpdateClientPayload;

            async fn build_create_client_message(
                &self,
                payload: Self::CreateClientPayload,
            ) -> $crate::_mercury_core::error::Result<Self::Message> {
                self.0.build_create_client_message(payload).await
            }

            async fn build_update_client_message(
                &self,
                client_id: &Self::ClientId,
                payload: Self::UpdateClientPayload,
            ) -> $crate::_mercury_core::error::Result<
                $crate::builders::UpdateClientOutput<Self::Message>,
            > {
                self.0
                    .build_update_client_message(client_id, payload)
                    .await
            }

            async fn build_register_counterparty_message(
                &self,
                client_id: &Self::ClientId,
                counterparty_client_id: &<$Inner as $crate::ChainTypes>::ClientId,
                counterparty_merkle_prefix: $crate::_mercury_core::MerklePrefix,
            ) -> $crate::_mercury_core::error::Result<Self::Message> {
                self.0
                    .build_register_counterparty_message(
                        client_id,
                        counterparty_client_id,
                        counterparty_merkle_prefix,
                    )
                    .await
            }

            fn enrich_update_payload(
                &self,
                payload: &mut Self::UpdateClientPayload,
                proofs: &[$crate::_mercury_core::MembershipProofEntry],
            ) {
                self.0.enrich_update_payload(payload, proofs);
            }

            fn finalize_batch(
                &self,
                update_output: &mut $crate::builders::UpdateClientOutput<Self::Message>,
                packet_messages: &mut [Self::Message],
            ) {
                self.0.finalize_batch(update_output, packet_messages);
            }

            async fn build_upgrade_client_message(
                &self,
                client_id: &Self::ClientId,
                payload: $crate::builders::UpgradeClientPayload,
            ) -> $crate::_mercury_core::error::Result<Vec<Self::Message>> {
                self.0
                    .build_upgrade_client_message(client_id, payload)
                    .await
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::builders::PacketMessageBuilder<$Inner> for $Wrapper
        where
            $Inner: $crate::builders::PacketMessageBuilder<$Inner>,
        {
            async fn build_receive_packet_message(
                &self,
                packet: &<$Inner as $crate::IbcTypes>::Packet,
                proof: <$Inner as $crate::IbcTypes>::CommitmentProof,
                proof_height: <$Inner as $crate::ChainTypes>::Height,
                revision: u64,
            ) -> $crate::_mercury_core::error::Result<Self::Message> {
                self.0
                    .build_receive_packet_message(packet, proof, proof_height, revision)
                    .await
            }

            async fn build_ack_packet_message(
                &self,
                packet: &<$Inner as $crate::IbcTypes>::Packet,
                ack: &<$Inner as $crate::IbcTypes>::Acknowledgement,
                proof: <$Inner as $crate::IbcTypes>::CommitmentProof,
                proof_height: <$Inner as $crate::ChainTypes>::Height,
                revision: u64,
            ) -> $crate::_mercury_core::error::Result<Self::Message> {
                self.0
                    .build_ack_packet_message(packet, ack, proof, proof_height, revision)
                    .await
            }

            async fn build_timeout_packet_message(
                &self,
                packet: &Self::Packet,
                proof: <$Inner as $crate::IbcTypes>::CommitmentProof,
                proof_height: <$Inner as $crate::ChainTypes>::Height,
                revision: u64,
            ) -> $crate::_mercury_core::error::Result<Self::Message> {
                self.0
                    .build_timeout_packet_message(packet, proof, proof_height, revision)
                    .await
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::builders::MisbehaviourDetector<$Inner> for $Wrapper
        where
            $Inner: $crate::builders::MisbehaviourDetector<$Inner>,
        {
            type UpdateHeader =
                <$Inner as $crate::builders::MisbehaviourDetector<$Inner>>::UpdateHeader;
            type MisbehaviourEvidence =
                <$Inner as $crate::builders::MisbehaviourDetector<$Inner>>::MisbehaviourEvidence;
            type CounterpartyClientState =
                <$Inner as $crate::builders::MisbehaviourDetector<$Inner>>::CounterpartyClientState;

            async fn check_for_misbehaviour(
                &self,
                client_id: &<$Inner as $crate::ChainTypes>::ClientId,
                update_header: &Self::UpdateHeader,
                client_state: &Self::CounterpartyClientState,
            ) -> $crate::_mercury_core::error::Result<Option<Self::MisbehaviourEvidence>> {
                self.0
                    .check_for_misbehaviour(client_id, update_header, client_state)
                    .await
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::builders::MisbehaviourMessageBuilder<$Inner> for $Wrapper
        where
            $Inner: $crate::builders::MisbehaviourMessageBuilder<$Inner>,
        {
            type MisbehaviourEvidence =
                <$Inner as $crate::builders::MisbehaviourMessageBuilder<$Inner>>::MisbehaviourEvidence;

            async fn build_misbehaviour_message(
                &self,
                client_id: &Self::ClientId,
                evidence: Self::MisbehaviourEvidence,
            ) -> $crate::_mercury_core::error::Result<Self::Message> {
                self.0
                    .build_misbehaviour_message(client_id, evidence)
                    .await
            }
        }

        #[$crate::_async_trait::async_trait]
        impl<$($gen)*> $crate::queries::MisbehaviourQuery<$Inner> for $Wrapper
        where
            $Inner: $crate::queries::MisbehaviourQuery<$Inner>,
        {
            type CounterpartyUpdateHeader =
                <$Inner as $crate::queries::MisbehaviourQuery<$Inner>>::CounterpartyUpdateHeader;

            async fn query_consensus_state_heights(
                &self,
                client_id: &Self::ClientId,
            ) -> $crate::_mercury_core::error::Result<Vec<Self::Height>> {
                self.0.query_consensus_state_heights(client_id).await
            }

            async fn query_update_client_header(
                &self,
                client_id: &Self::ClientId,
                consensus_height: &Self::Height,
            ) -> $crate::_mercury_core::error::Result<Option<Self::CounterpartyUpdateHeader>> {
                self.0
                    .query_update_client_header(client_id, consensus_height)
                    .await
            }
        }
    };

    (@cpb [$($gen:tt)+] $Wrapper:ty, $Inner:ty) => {
        #[$crate::_async_trait::async_trait]
        impl<$($gen)+, __MerC: $crate::ChainTypes> $crate::builders::ClientPayloadBuilder<__MerC>
            for $Wrapper
        where
            $Inner: $crate::builders::ClientPayloadBuilder<__MerC>,
        {
            type CreateClientPayload =
                <$Inner as $crate::builders::ClientPayloadBuilder<__MerC>>::CreateClientPayload;
            type UpdateClientPayload =
                <$Inner as $crate::builders::ClientPayloadBuilder<__MerC>>::UpdateClientPayload;

            async fn build_create_client_payload(
                &self,
            ) -> $crate::_mercury_core::error::Result<Self::CreateClientPayload> {
                self.0.build_create_client_payload().await
            }

            async fn build_update_client_payload(
                &self,
                trusted_height: &Self::Height,
                target_height: &Self::Height,
                counterparty_client_state: &<__MerC as $crate::IbcTypes>::ClientState,
            ) -> $crate::_mercury_core::error::Result<Self::UpdateClientPayload>
            where
                __MerC: $crate::IbcTypes,
            {
                self.0
                    .build_update_client_payload(
                        trusted_height,
                        target_height,
                        counterparty_client_state,
                    )
                    .await
            }

            fn update_payload_proof_height(
                &self,
                payload: &Self::UpdateClientPayload,
            ) -> Option<Self::Height> {
                self.0.update_payload_proof_height(payload)
            }

            fn update_payload_message_height(
                &self,
                payload: &Self::UpdateClientPayload,
            ) -> Option<Self::Height> {
                self.0.update_payload_message_height(payload)
            }

            fn required_dst_timestamp_secs(
                &self,
                payload: &Self::UpdateClientPayload,
            ) -> Option<u64> {
                self.0.required_dst_timestamp_secs(payload)
            }

            async fn build_upgrade_client_payload(
                &self,
            ) -> $crate::_mercury_core::error::Result<
                Option<$crate::builders::UpgradeClientPayload>,
            > {
                self.0.build_upgrade_client_payload().await
            }
        }
    };
    (@cpb [] $Wrapper:ty, $Inner:ty) => {
        #[$crate::_async_trait::async_trait]
        impl<__MerC: $crate::ChainTypes> $crate::builders::ClientPayloadBuilder<__MerC>
            for $Wrapper
        where
            $Inner: $crate::builders::ClientPayloadBuilder<__MerC>,
        {
            type CreateClientPayload =
                <$Inner as $crate::builders::ClientPayloadBuilder<__MerC>>::CreateClientPayload;
            type UpdateClientPayload =
                <$Inner as $crate::builders::ClientPayloadBuilder<__MerC>>::UpdateClientPayload;

            async fn build_create_client_payload(
                &self,
            ) -> $crate::_mercury_core::error::Result<Self::CreateClientPayload> {
                self.0.build_create_client_payload().await
            }

            async fn build_update_client_payload(
                &self,
                trusted_height: &Self::Height,
                target_height: &Self::Height,
                counterparty_client_state: &<__MerC as $crate::IbcTypes>::ClientState,
            ) -> $crate::_mercury_core::error::Result<Self::UpdateClientPayload>
            where
                __MerC: $crate::IbcTypes,
            {
                self.0
                    .build_update_client_payload(
                        trusted_height,
                        target_height,
                        counterparty_client_state,
                    )
                    .await
            }

            fn update_payload_proof_height(
                &self,
                payload: &Self::UpdateClientPayload,
            ) -> Option<Self::Height> {
                self.0.update_payload_proof_height(payload)
            }

            fn update_payload_message_height(
                &self,
                payload: &Self::UpdateClientPayload,
            ) -> Option<Self::Height> {
                self.0.update_payload_message_height(payload)
            }

            fn required_dst_timestamp_secs(
                &self,
                payload: &Self::UpdateClientPayload,
            ) -> Option<u64> {
                self.0.required_dst_timestamp_secs(payload)
            }

            async fn build_upgrade_client_payload(
                &self,
            ) -> $crate::_mercury_core::error::Result<
                Option<$crate::builders::UpgradeClientPayload>,
            > {
                self.0.build_upgrade_client_payload().await
            }
        }
    };
}
