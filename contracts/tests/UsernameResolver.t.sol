// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {Test} from "forge-std-1.8.2/Test.sol";
import {Ownable, UsernameResolver} from "../UsernameResolver.sol";
import {IExternalResolver} from "../interfaces/IExternalResolver.sol";

contract UsernameResolverTest is Test {
    UsernameResolver public resolver;

    uint256 gatewaySig;
    address gateway;

    uint256 userSig;
    address user;

    function setUp() public {
        (gateway, gatewaySig) = makeAddrAndKey("Gateway");
        (user, userSig) = makeAddrAndKey("User");

        address[] memory signers = new address[](1);
        signers[0] = gateway;

        resolver = new UsernameResolver("http://example.com/", signers);

        vm.label(address(resolver), "UsernameResolver");
    }

    function testResolveInitiatesOffchainLookup() public {
        string[] memory urls = new string[](1);
        urls[0] = resolver.gatewayURL();

        bytes memory callData = abi.encodeWithSelector(
            IExternalResolver.resolve.selector,
            bytes("name"),
            bytes("data")
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                UsernameResolver.OffchainLookup.selector,
                address(resolver),
                urls,
                callData,
                UsernameResolver.resolveWithProof.selector,
                abi.encode(callData, address(resolver))
            )
        );
        resolver.resolve("name", "data");
    }

    function testResolveWithProofReturnsGatewayData() public view {
        bytes memory requestCallData = abi.encodeWithSelector(
            IExternalResolver.resolve.selector,
            bytes("name"),
            bytes("data")
        );

        bytes memory responseData = bytes("some-data");

        bytes memory contractResponse = resolver.resolveWithProof(
            abi.encode(
                responseData,
                block.timestamp + 3600,
                _generateGatewaySignature(
                    gatewaySig,
                    address(resolver),
                    block.timestamp + 3600,
                    keccak256(requestCallData),
                    keccak256(responseData)
                )
            ),
            abi.encode(requestCallData, address(resolver))
        );

        assertEq(responseData, contractResponse);
    }

    function testResolveWithProofValidatesExpiration() public {
        bytes memory requestCallData = abi.encodeWithSelector(
            IExternalResolver.resolve.selector,
            bytes("name"),
            bytes("data")
        );

        bytes memory responseData = bytes("some-data");
        uint256 expiresAt = block.timestamp + 3600;

        bytes memory sig = _generateGatewaySignature(
            gatewaySig,
            address(resolver),
            expiresAt,
            keccak256(requestCallData),
            keccak256(responseData)
        );

        vm.warp(expiresAt + 1);

        vm.expectRevert(UsernameResolver.SignatureExpired.selector);
        resolver.resolveWithProof(
            abi.encode(responseData, expiresAt, sig),
            abi.encode(requestCallData, address(resolver))
        );
    }

    function testResolveWithProofValidatesSigner() public {
        bytes memory requestCallData = abi.encodeWithSelector(
            IExternalResolver.resolve.selector,
            bytes("name"),
            bytes("data")
        );

        bytes memory responseData = bytes("some-data");

        bytes memory sig = _generateGatewaySignature(
            userSig,
            address(resolver),
            block.timestamp + 3600,
            keccak256(requestCallData),
            keccak256(responseData)
        );

        vm.expectRevert(UsernameResolver.InvalidSignature.selector);
        resolver.resolveWithProof(
            abi.encode(responseData, block.timestamp + 3600, sig),
            abi.encode(requestCallData, address(resolver))
        );
    }

    function testResolveWithProofValidatesSignature() public {
        bytes memory requestCallData = abi.encodeWithSelector(
            IExternalResolver.resolve.selector,
            bytes("name"),
            bytes("data")
        );

        bytes memory responseData = bytes("some-data");

        bytes memory sig = _generateGatewaySignature(
            gatewaySig,
            address(resolver),
            block.timestamp + 3600,
            keccak256(requestCallData),
            keccak256("different-data")
        );

        vm.expectRevert(UsernameResolver.InvalidSignature.selector);
        resolver.resolveWithProof(
            abi.encode(responseData, block.timestamp + 3600, sig),
            abi.encode(requestCallData, address(resolver))
        );
    }

    function testSupportsRequiredInterfaces() public view {
        assertTrue(resolver.supportsInterface(0x01ffc9a7)); // ERC165
        assertTrue(resolver.supportsInterface(0x9061b923)); // IExtendedResolver
    }

    function testCanUpdateSigner() public {
        assertFalse(resolver.canSignResponses(user));
        assertTrue(resolver.canSignResponses(gateway));

        // only owner can call the function
        vm.prank(user);
        vm.expectRevert(Ownable.Unauthorized.selector);
        resolver.setSigner(user, true);

        // owner can update signers
        resolver.setSigner(user, true);
        resolver.setSigner(gateway, false);

        assertTrue(resolver.canSignResponses(user));
        assertFalse(resolver.canSignResponses(gateway));
    }

    function testCanModifyGatewayUrl() public {
        assertEq(resolver.gatewayURL(), "http://example.com/");

        // only owner can call the function
        vm.prank(user);
        vm.expectRevert(Ownable.Unauthorized.selector);
        resolver.setGatewayURL("https://gateway.example");

        // owner can update the gateway url
        resolver.setGatewayURL("https://gateway.example");
        assertEq(resolver.gatewayURL(), "https://gateway.example");
    }

    function testCanTransferOwnership() public {
        assertEq(resolver.owner(), address(this));

        vm.prank(user);
        resolver.requestOwnershipHandover();

        // only owner can call the function
        vm.prank(user);
        vm.expectRevert(Ownable.Unauthorized.selector);
        resolver.completeOwnershipHandover(user);

        // owner can transfer ownership
        resolver.completeOwnershipHandover(user);
        assertEq(resolver.owner(), user);
    }

    function testCannotTransferOwnershipInOneStep() public {
        // only owner can call the function
        vm.prank(user);
        vm.expectRevert(Ownable.Unauthorized.selector);
        resolver.transferOwnership(user);

        // owner cannot tranfer ownership in one step
        vm.expectRevert(UsernameResolver.CannotRenounceOwnership.selector);
        resolver.transferOwnership(user);
    }

    function testCannotRenounceOwnership() public {
        // only owner can call the function
        vm.prank(user);
        vm.expectRevert(Ownable.Unauthorized.selector);
        resolver.renounceOwnership();

        // owner cannot renounce ownership
        vm.expectRevert(UsernameResolver.CannotRenounceOwnership.selector);
        resolver.renounceOwnership();
    }

    /// @dev Helper function to generate the gateway's signature
    /// @param privateKey The gateway's private key
    /// @param sender The address of the sender
    /// @param expiresAt The expiration time of the signature
    /// @param requestHash The hash of the request
    /// @param responseHash The hash of the response
    function _generateGatewaySignature(
        uint256 privateKey,
        address sender,
        uint256 expiresAt,
        bytes32 requestHash,
        bytes32 responseHash
    ) internal view returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(
            privateKey,
            _hashTypedDataV4(
                keccak256(
                    abi.encode(
                        keccak256(
                            "GatewayResponse(address sender,uint256 expiresAt,bytes32 requestHash,bytes32 responseHash)"
                        ),
                        sender,
                        expiresAt,
                        requestHash,
                        responseHash
                    )
                )
            )
        );

        return abi.encodePacked(r, s, v);
    }

    /// @dev Helper function to hash EIP712 typed data for the gateway signature
    /// @param structHash The hash of the GatewayResponse struct
    function _hashTypedDataV4(
        bytes32 structHash
    ) internal view returns (bytes32) {
        return
            keccak256(
                abi.encodePacked(
                    "\x19\x01",
                    resolver.DOMAIN_SEPARATOR(),
                    structHash
                )
            );
    }
}
