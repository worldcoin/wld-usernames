## World Usernames
This is our open source implementation of ENS compatible Usernames

# 🚀 Running Locally
```
cp .env.example .env
docker compose up --detach
sqlx migrate run

cargo run

// go to localhost:8000
```

# Updating Queries
In order to update the queries, you need to run the following command:
```
cargo sqlx prepare
```

# 🛳️ Finding Deployments
[Production Deployment](https://usernames.worldcoin.org/docs)
[ENS Resolver](https://etherscan.io/address/0xB4E36A6C3403137d8fdaf4e91b91D1aBC2caF3Dd)
