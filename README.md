## World Usernames

This is our open source implementation of ENS compatible Usernames

# 🚀 Running Locally

### Generate certs

All fields can be left blank

```
cd certs;
openssl genrsa -out ca.key 2048;
openssl req -new -x509 -days 365 -key ca.key -out ca.crt;
openssl genrsa -out redis.key 2048;
openssl req -new -key redis.key -out redis.csr;
openssl x509 -req -days 365 -in redis.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out redis.crt;
cd -;
```

```
cp .env.example .env
docker compose up --detach

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
