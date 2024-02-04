cargo run -- -p 3000 --id 1 >> logs/node_3000.txt 2>&1 &
cargo run -- -p 3001 --id 2 >> logs/node_3001.txt 2>&1 &
cargo run -- -p 3002 --id 3 >> logs/node_3002.txt 2>&1 &

sleep 3

curl -X POST http://0.0.0.0:3000/connect -d "3001"
curl -X POST http://0.0.0.0:3000/connect -d "3002"
curl -X POST http://0.0.0.0:3001/connect -d "3002"
