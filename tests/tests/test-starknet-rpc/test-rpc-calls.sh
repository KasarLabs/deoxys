#!/bin/bash

URL1="http://127.0.0.1:9944/"
URL2="https://starknet-mainnet.g.alchemy.com/v2/hnj_DGevqpyoyeoEs9Vfx-6qSTHOnaIu"

declare -a Calls=(
       "starknet_getBlockWithTxHashes [\"latest\"]"
       "starknet_getBlockWithTxs {\"block_id\":\"latest\"}"
       "starknet_getStateUpdate {\"block_id\":\"latest\"}"
       "starknet_getStorageAt [\"0x0000000000000000000000000000000000000000000000000000000000001111\",\"0x02900ac0f31b4cd8101abb46a91021989eb7c9f6e2a5417186e476f08429efce\",\"latest\"]" # Replace with valid address and position
       "starknet_getTransactionByHash {\"transaction_hash\":\"0x04698fe07eaf8548c8d47134131f2ef037a8feac3e5fd996f1dfdf7fa06868e3\"}"
       "starknet_getTransactionByBlockIdAndIndex {\"block_id\":\"latest\",\"index\": 0}"
       "starknet_getClass [\"latest\", \"0x00d0e183745e9dae3e4e78a8ffedcce0903fc4900beace4e0abf192d4c202da3\"]"
       "starknet_getTransactionReceipt {\"transaction_hash\":\"0x4c1672e824b5cd7477fca31ee3ab5a1058534ed1820bb27abc976c2e6095151\"}"
       "starknet_getClassHashAt [\{\"block_number\": \22050\},\"0x07f38ab7537dbb5f8dc2d049d441f2b250c2186a13d943b8467cfa86b8dba12b\"]" # Replace with valid address and position
       "starknet_getClassAt {\"block_id\":\"50000\", \"contract_address\":\"0x07e2a13b40fc1119ec55e0bcf9428eedaa581ab3c924561ad4e955f95da63138\"}" # Replace with valid address and position
       "starknet_getBlockTransactionCount {\"block_hash\":\"0x053315a56543737cd1b2dc40c60e84d03a9b10d712c9b29f488dc979f0cd56bd\"}"
       "starknet_call {\"to\":\"0x123\", \"data\":\"0x123\"}" # Replace with valid address and data
       "starknet_estimateFee {\"to\":\"0x123\", \"value\":\"0x123\", \"gasPrice\":\"0x123\", \"gas\":\"0x123\", \"data\":\"0x123\"}" # Replace with valid address, value, gasPrice, gas, data
       "starknet_estimateMessageFee {\"to\":\"0x123\", \"value\":\"0x123\", \"gasPrice\":\"0x123\", \"gas\":\"0x123\", \"data\":\"0x123\"}" # Replace with valid address, value, gasPrice, gas, data
       "starknet_blockNumber {}"
       "starknet_blockHashAndNumber {\"block_number\":\"50000\"}"
       "starknet_chainId {}")


for call in "${Calls[@]}"; do
    IFS=' ' read -r -a parts <<< "$call"
    method="${parts[0]}"
    params="${parts[1]}"

    echo -e "\033[0;36mTesting endpoint: $method \033[0m"

    echo -e "\033[0;33mURL1: $URL1 \033[0m"
    response1=$(curl -s -X POST $URL1 \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc":"2.0",
            "method":"'$method'",
            "params":'$params',
            "id":1
        }')

    echo -e "\033[0;33mURL2: $URL2 \033[0m"
    response2=$(curl -s -X POST $URL2 \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc":"2.0",
            "method":"'$method'",
            "params":'$params',
            "id":1
        }')

    # Compare responses
   if [[ "$response1" == *'"error":'* && "$response2" == *'"result":'* ]]; then
    echo -e "\033[0;31m❌ $method - Different response in URLs \033[0m"
    echo -e "Response in URL1 (Local RPC):"
    echo "$response1" | jq -c .
    echo -e "Response in URL2 (Official RPC Provider):"
    echo "$response2" | jq -c .
elif [[ "$response1" == *'"result":'* && "$response2" == *'"error":'* ]]; then
    echo -e "\033[0;31m❌ $method - Different response in URLs \033[0m"
    echo -e "Response in URL1 (Local RPC):"
    echo "$response1" | jq -c .
    echo -e "Response in URL2 (Official RPC Provider):"
    echo "$response2" | jq -c .
elif [[ "$response1" == *'"error":'* && "$response2" == *'"error":'* ]]; then
    # Both URLs have parse error
    echo -e "\033[0;31m❌ $method - Parse error in both URLs \033[0m"
else
    # Valid comparison when both URLs have a "result" field
    echo -e "\033[0;32m✅ $method - Same response in both URLs \033[0m"
fi


    echo -e "\n"
done
