# MVP Doc
The idea is to build a peer to peer cloud compute market, that is accessible to anyone as a renter / provider and fully permissionless.
Providers can offer their hardware and credentials are generated & issued to users on first successful payment. All users pay all providers
every 1 minute, via automated Tia payments on Celestia's Mocha testnet. A contract is cancelled automatically and credentials are re-set after
X amount of time has passed with no payment. 

X here is set by the provider and transparently advertised in the market UI. X should be large enough
to account for temporary outages; the payment service that the users installs will automatically pay the incurred amount in such a case e.g. keep
track of the debt and pay it in full even if a payment fails. No exponential backoff, because that would risk double-payment and unnecessary gas costs.
E.g. if it failed for 10 minutes, then a single transaction of 10 minutes worth of debt is sent, not 10 individual ones.

# Design philosophy

This app is very different, but it should follow the design philosophy we have established for tee-ism app in Desktop/interop/tee-ism/*. The color scheme codes you find on a screenshot on Desktop.

# Initial Implementation
This is essentially a React app site with a Rust backend / router. I should be able to deploy the whole stack as a service on a linux server by running ./install.sh and it should be served on that server
at the specified port in the install script. There should be instructions that are clear and straightforward to offer a server for rent e.g. linux instructions on how to set the server up for credential
generation and rotation e.g. auto-managed renting with tia payments (to some address the person selling specifices). The renter should know how to connect to the server he rented and how to start paying it.
For each offer on the market the first payment on-chain closes the deal / initiates the rent. We also need to address the problem where 2 people attempt to rent at the same time, ideally by routing the money through and app controled account that can refund.
The first version should run on celestia mocha and connect to a working RPC (find one). The install script should install and run everything as a systemd and re-running the install script will update if code
has changed.

User's don't create accounts. They just connect their Keplr wallet and their celestia on-chain ID is their account effectively. Server is mostly stateless; but you can have a small sqlite db for necessary 
state.

# UI sections
market | getting started | manage agents | account

market: discover & rent new servers
getting started: docs on how to list a server for rent or rent one and pay the TIA fees
manage agents: credentials & currently owned contracts
account: balance, burn rate, projection, ...

# Server rent
I will give you ssh access & password for a single server that you are allowed to use as the first market listing & for development.
The server should be listed for 0.00001 TIA per minute as a base cost such that I can play with the whole system and rent/unrent it many times with 1 TIA on mocha.
When a user stops paying / cancels the contract, then the server should be automatically re-set e.g. the user's data should be fully deleted.
Maybe we should only allow users to run docker images or in a VM or sth; that's up to you. Ideally secure by default and fully pruned when user rotates.

this is the server you can use for experimentation: ssh ubuntu@195.154.103.162
it doesn't have the password but my default ssh key in my environment is whitelisted so you are allowed to use that.

# MVP deployment
You first develop the whole thing here locally using that server 195.154... as the first on the market
I then deploy it to my ark server that already runs an interop stack - so it is VERY important that install.sh does nothing aggressive and just starts the app nicely and seamlessly on the specified port.
The ark server runs a react app already that must not break and uses several ports that we should not collide with. I don't want to break anything on ark and just add this as another app on ark.
But we'll get to that in the end. I'll tell you when I feel it's ready for deployment on ark. For now just local. 
