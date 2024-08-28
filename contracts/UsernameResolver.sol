// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {ECDSA} from "solady-0.0.210/utils/ECDSA.sol";
import {EIP712} from "solady-0.0.210/utils/EIP712.sol";
import {Ownable} from "solady-0.0.210/auth/Ownable.sol";
import {IExternalResolver} from "./interfaces/IExternalResolver.sol";

/// @title World App Resolver
/// @author Miguel Piedrafita
/// @notice An off-chain ENS resolver mapping World App usernames to ENS subdomains
contract UsernameResolver is Ownable, EIP712 {
    ///////////////////////////////////////////////////////////////////////////////
    ///                                  ERRORS                                ///
    //////////////////////////////////////////////////////////////////////////////

    /// @notice An error thrown to indicate that the received signature is invalid
    error InvalidSignature();

    /// @notice Thrown when the owner attempts to resign ownership or skip the two-step ownership transfer process.
    error CannotRenounceOwnership();

    /// @notice An error thrown to indicate that the received signature has expired
    error SignatureExpired();

    /// @notice An error thrown to indicate that the client should perform an off-chain lookup
    /// @param sender The address of the contract that raised the error
    /// @param urls A list of gateway URLs that can answer the query
    /// @param callData The request callData to be sent to the gateway
    /// @param callbackFunction The function that will receive the gateway's response, as a 4-byte selector
    /// @param extraData Additional data required by the callback function
    /// @dev See EIP-3668 for more details.
    error OffchainLookup(
        address sender,
        string[] urls,
        bytes callData,
        bytes4 callbackFunction,
        bytes extraData
    );

    ///////////////////////////////////////////////////////////////////////////////
    ///                              CONFIG STORAGE                            ///
    //////////////////////////////////////////////////////////////////////////////

    /// @notice The URL of the gateway that will answer off-chain lookup requests
    string public gatewayURL;

    /// @notice A mapping of addresses to a boolean value indicating whether they are authorized to sign messages
    mapping(address => bool) public canSignResponses;

    ///////////////////////////////////////////////////////////////////////////////
    ///                               CONSTRUCTOR                              ///
    //////////////////////////////////////////////////////////////////////////////

    /// @notice Create a new UsernameResolver
    /// @param _gatewayURL The URL of the gateway that will answer off-chain lookup requests
    /// @param _signers A list of addresses that are authorized to sign messages
    constructor(string memory _gatewayURL, address[] memory _signers) {
        gatewayURL = _gatewayURL;
        _initializeOwner(msg.sender);

        uint256 signersLength = _signers.length;
        for (uint256 i = 0; i < signersLength; ) {
            canSignResponses[_signers[i]] = true;
            unchecked {
                ++i;
            }
        }
    }

    ///////////////////////////////////////////////////////////////////////////////
    ///                              RESOLVER LOGIC                             ///
    //////////////////////////////////////////////////////////////////////////////

    /// @notice Resolves a name, as specified by ENSIP 10.
    /// @param name The DNS-encoded name to resolve.
    /// @param data The ABI encoded data for the underlying resolution function (Eg, addr(bytes32), text(bytes32,string), etc).
    /// @return result The return data, ABI encoded identically to the underlying function.
    function resolve(
        bytes calldata name,
        bytes calldata data
    ) external view returns (bytes memory) {
        bytes memory callData = abi.encodeWithSelector(
            IExternalResolver.resolve.selector,
            name,
            data
        );

        string[] memory urls = new string[](1);
        urls[0] = gatewayURL;

        revert OffchainLookup(
            address(this),
            urls,
            callData,
            UsernameResolver.resolveWithProof.selector,
            abi.encode(callData, address(this))
        );
    }

    /// @notice Callback used by CCIP read compatible clients to verify and parse the response.
    /// @param response The response data, ABI encoded identically to the underlying function.
    /// @param extraData The extra data required to verify the response.
    /// @return result The return data, ABI encoded identically to the underlying function.
    function resolveWithProof(
        bytes calldata response,
        bytes calldata extraData
    ) external view returns (bytes memory) {
        (
            bytes memory responseData,
            uint64 expiresAt,
            bytes memory signature
        ) = abi.decode(response, (bytes, uint64, bytes));

        (bytes memory requestData, address sender) = abi.decode(
            extraData,
            (bytes, address)
        );

        address signer = ECDSA.recover(
            _hashTypedData(
                keccak256(
                    abi.encode(
                        keccak256(
                            "GatewayResponse(address sender,uint256 expiresAt,bytes32 requestHash,bytes32 responseHash)"
                        ),
                        sender,
                        expiresAt,
                        keccak256(requestData),
                        keccak256(responseData)
                    )
                )
            ),
            signature
        );

        if (!canSignResponses[signer]) revert InvalidSignature();
        if (expiresAt < block.timestamp) revert SignatureExpired();

        return responseData;
    }

    /// @notice Whether this contract implements the interface defined by `interfaceID`.
    /// @param interfaceID The interface identifier, as defined in ERC-165.
    /// @return result `true` if this contract implements the interface defined by `interfaceID`, `false` otherwise.
    function supportsInterface(
        bytes4 interfaceID
    ) public pure virtual returns (bool result) {
        /// @solidity memory-safe-assembly
        assembly {
            let s := shr(224, interfaceID)
            // ERC165: 0x01ffc9a7, IExtendedResolver: 0x9061b923
            result := or(eq(s, 0x01ffc9a7), eq(s, 0x9061b923))
        }
    }

    /// @dev The EIP-712 domain separator
    /// @return separator The EIP-712 domain separator
    function DOMAIN_SEPARATOR() external view returns (bytes32 separator) {
        separator = _domainSeparator();
    }

    ///////////////////////////////////////////////////////////////////////////////
    ///                               CONFIG LOGIC                             ///
    //////////////////////////////////////////////////////////////////////////////

    /// @notice Adds or removes an address fom the list of authorized signers
    /// @param signer The address to add or remove
    /// @param enabled Whether to add or remove the signer
    /// @dev Can only be called by the owner
    function setSigner(address signer, bool enabled) external onlyOwner {
        canSignResponses[signer] = enabled;
    }

    /// @notice Sets the gateway URL
    /// @param _gatewayURL The new gateway URL
    /// @dev Can only be called by the owner
    function setGatewayURL(string memory _gatewayURL) external onlyOwner {
        gatewayURL = _gatewayURL;
    }

    /// @notice Prevents the owner from transferring ownership without going through the two-step process
    /// @dev Can only be called by the owner
    function transferOwnership(address) public payable override onlyOwner {
        revert CannotRenounceOwnership();
    }

    /// @notice Prevents the owner from renouncing ownership
    /// @dev Can only be called by the owner
    function renounceOwnership() public payable override onlyOwner {
        revert CannotRenounceOwnership();
    }

    ///////////////////////////////////////////////////////////////////////////////
    ///                              INTERNAL LOGIC                            ///
    //////////////////////////////////////////////////////////////////////////////

    function _domainNameAndVersion()
        internal
        pure
        override
        returns (string memory name, string memory version)
    {
        name = "World App Usernames";
        version = "1";
    }
}
