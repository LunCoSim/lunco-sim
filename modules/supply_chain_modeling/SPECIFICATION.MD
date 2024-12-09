# Specification for Resource and Supply Chain Modeling

## Introduction

LunCo Resource and Supply Chain Modeling Tool (RSCT) is a module that allows users to create high-level resource and supply chain models and visualize them in a graph views.

Possible use cases:

- Lunar Base Food consumption
- Lunar Base Energy consumption
- Lunar Base Water consumption
- Lunar Base Carbon footprint
- Lunar Base Supply chain optimization

1. Time
2. Entities:
    - Resources - e.g. water, electricity, etc. 
    - Objects - e.g. factory, warehouse, etc.
    - Relationships - e.g. production, consumption, etc.
    - Properties - e.g. mass, volume, etc.

It has to be implemented in a way that allows to create new resources, objects, relationships, and properties in Godot 4.4+

### Resource Specification

1. Resource - could be a product, service, or any other resource that can be used in a supply chain. There would be set of build in + users should be able to define their own resources.

### Object Specification

1. Objects that consumes resources and objects that produce resources.  
2. Object has inputs and outputs, inputs and outputs are resources.
3. Objects has parameters

### Relationship Specification

1. Relationship is a connection between two objects representing a flow of resources.
2. Relationship has a direction, which is the direction of the flow of resources.
3. Relationship has a type, which is the type of the relationship.
4. There could be other types of relationships, e.g. ownership, etc.

## Onchain

To make it easier to share design we propose to create a ledger of models, resources, objects, relationships, and properties onchain

1. Publish the model to the blockchain
2. Publish resources/objects as NFTs
3. Ledger for resources/objects, simulations
4. Fraud protection via staking + community moderation on chain
5. Auction for higher position - the more you stake the higher your position
6. Separate views for published models, makes sense to prototype only complete simulation NFT

## General specs

1. The model should track it state and visualise it in a graph view.
2. The model should be able to simulate the supply chain in time
