// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

interface IExternalResolver {
    function resolve(
        bytes calldata name,
        bytes calldata data
    )
        external
        view
        returns (bytes memory response, uint64 expires, bytes memory sig);
}
