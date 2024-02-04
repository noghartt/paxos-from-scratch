PORTS=(3000, 3001, 3002)

for PORT in "${PORTS[@]}"; do
    PID=$(lsof -ti tcp:$PORT)
    if [ ! -z "$PID" ]; then
        echo "Killing process $PID on port $PORT"
        kill $PID
    else
        echo "No process found on port $PORT"
    fi
done

echo "All specified ports have been processed."

rm -rf ./logs/*
