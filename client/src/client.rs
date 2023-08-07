use crate::utils;
use crate::utils::greeting_public_key;
use crate::{Error, Result};
use solana_program::pubkey::Pubkey;
use utils::ACTION;
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::signature::Signer;
use solana_sdk::signer::keypair::{read_keypair_file, Keypair};
use solana_sdk::transaction::Transaction;

/// Establishes a RPC connection with the solana cluster configured by
/// `solana config set --url <URL>`. Information about what cluster
/// has been configured is gleened from the solana config file
/// `~/.config/solana/cli/config.yml`.
pub fn establish_connection() -> Result<RpcClient> {
    let rpc_url = utils::get_rpc_url()?;
    Ok(RpcClient::new_with_commitment(
        rpc_url,
        CommitmentConfig::confirmed(),
    ))
}

/// Determines the amount of lamports that will be required to execute
/// this smart contract. The minimum balance is calculated assuming
/// that the user would like to make their account rent exempt.
///
/// For more information about rent see the Solana documentation
/// [here](https://docs.solana.com/implemented-proposals/rent#two-tiered-rent-regime)
pub fn get_balance_requirement(connection: &RpcClient) -> Result<u64> {
    let account_fee =
        connection.get_minimum_balance_for_rent_exemption(utils::get_greeting_data_size()?)?;

    let (_, fee_calculator) = connection.get_recent_blockhash()?;
    let transaction_fee = fee_calculator.lamports_per_signature * 100;

    Ok(transaction_fee + account_fee)
}

/// Gets the balance of PLAYER in lamports via a RPC call over
/// CONNECTION.
pub fn get_player_balance(player: &Keypair, connection: &RpcClient) -> Result<u64> {
    Ok(connection.get_balance(&player.pubkey())?)
}

/// Requests that AMOUNT lamports are transfered to PLAYER via a RPC
/// call over CONNECTION.
///
/// Airdrops are only avaliable on test networks.
pub fn request_airdrop(player: &Keypair, connection: &RpcClient, amount: u64) -> Result<()> {
    let sig = connection.request_airdrop(&player.pubkey(), amount)?;
    loop {
        let confirmed = connection.confirm_transaction(&sig)?;
        if confirmed {
            break;
        }
    }
    Ok(())
}

/// Loads keypair information from the file located at KEYPAIR_PATH
/// and then verifies that the loaded keypair information corresponds
/// to an executable account via CONNECTION. Failure to read the
/// keypair or the loaded keypair corresponding to an executable
/// account will result in an error being returned.
pub fn get_program(keypair_path: &str, connection: &RpcClient) -> Result<Keypair> {
    let program_keypair = read_keypair_file(keypair_path).map_err(|e| {
        Error::InvalidConfig(format!(
            "failed to read program keypair file ({}): ({})",
            keypair_path, e
        ))
    })?;

    let program_info = connection.get_account(&program_keypair.pubkey())?;
    if !program_info.executable {
        return Err(Error::InvalidConfig(format!(
            "program with keypair ({}) is not executable",
            keypair_path
        )));
    }

    Ok(program_keypair)
}

/// On Solana accounts are ways to store data. In order to use our
/// greeting counter smart contract we need some way to store the
/// number of times we have said hello to the contract. To do this we
/// create a greeting account which we subsequentally transfer
/// ownership of to the program. This allows the program to write to
/// that account as it deems fit.
///
/// The greeting account has a [derived
/// address](https://docs.solana.com/developing/programming-model/calling-between-programs#program-derived-addresses)
/// which allows it to own and manage the account. Additionally the
/// address being derived means that we can regenerate it when we'd
/// like to find the greeting account again later.
pub fn create_greeting_account(
    user: &Keypair,
    program: &Keypair,
    connection: &RpcClient,
) -> Result<()> {
    let greeting_pubkey = utils::greeting_public_key(&user.pubkey(), &program.pubkey())?;

    let mut success = false;
    if let Err(_) = connection.get_account(&greeting_pubkey) {
        println!("... creating greeting account");
        let lamport_requirement = connection.get_minimum_balance_for_rent_exemption(
            utils::get_greeting_data_size()?
        )?;
        println!("--- min_balance_for_rent_exemption: {}", lamport_requirement);
        let greeting_data_size = utils::get_greeting_data_size().unwrap() as u64;
        println!("--- greeting_data_size: {}", greeting_data_size);

        // This instruction creates an account with the key
        // "greeting_pubkey". The created account is owned by the
        // program. The account is loaded with enough lamports to stop
        // it from needing to pay rent. The lamports to fund this are
        // paid by the player.
        //
        // It is important that the program owns the created account
        // because it needs to be able to modify its contents.
        //
        // The address of the account created by
        // create_account_with_seed is the same as the address
        // generated by utils::get_greeting_public_key. We do this as
        // opposed to create_account because create account doesn't
        // derive that address like that.
        let instruction = solana_sdk::system_instruction::create_account_with_seed(
            &user.pubkey(),
            &greeting_pubkey,
            &user.pubkey(),
            utils::get_greeting_seed(),
            lamport_requirement,
            utils::get_greeting_data_size()? as u64,
            &program.pubkey(),
        );
        let message = Message::new(&[instruction], Some(&user.pubkey()));
        let transaction =
            Transaction::new(&[user], message, connection.get_recent_blockhash()?.0);

        let signature = connection.send_and_confirm_transaction(&transaction)?;
        success = true;
        println!("Signature: {}", signature);
    }

    if !success { println!("... not created, account may already exist "); }

    Ok(())
}

pub fn create_instruction(
    action: ACTION,
    data: u8,
    program: &Keypair,
    program_derived_account: Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        program.pubkey(),
        &[action as u8, data],
        vec![AccountMeta::new(program_derived_account, false)],
    )    
}

pub fn set_reputation(
    rep: u8,
    player: &Keypair,
    program: &Keypair,
    connection: &RpcClient,
) {
    let key = greeting_public_key(&player.pubkey(), &program.pubkey()).unwrap();
    let ins = create_instruction(ACTION::SetRep, rep, program, key);
    let result = send_action_tx(ins, player, connection);
    println!("--- set_reputation result: {:?}", result);
}

/// Sends an instruction from PLAYER to PROGRAM via CONNECTION. The
/// instruction contains no data but does contain the address of our
/// previously generated greeting account. The program will use that
/// passed in address to update its greeting counter after verifying
/// that it owns the account that we have passed in.
pub fn send_action_tx(
    instruction: Instruction,
    player: &Keypair, 
    connection: &RpcClient,
) -> Result<()> {
    // Submit an instruction to the chain which tells the program to
    // run. We pass the account that we want the results to be stored
    // in as one of the accounts arguments which the program will
    // handle.

    let message = Message::new(&[instruction], Some(&player.pubkey()));
    let transaction = Transaction::new(
        &[player], message, connection.get_recent_blockhash()?.0
    );
    connection.send_and_confirm_transaction(&transaction)?;

    Ok(())
}

pub fn get_greeting_obj(
    user: &Keypair, program: &Keypair, connection: &RpcClient
) -> Result<utils::ShopSchema> {
    let greeting_pubkey = utils::greeting_public_key(&user.pubkey(), &program.pubkey())?;
    let greeting_account = connection.get_account(&greeting_pubkey)?;
    println!("--- greeting_account_data: {:?}", &greeting_account.data);
    Ok(utils::get_greeting_obj(&greeting_account.data)?)
}