use core::convert::AsMut;
use rstd::result;

use primitives::Bytes;
use primitives::U256;
use primitives::convert_hash;
use runtime_primitives::traits::{As, Hash, Zero};

use support::StorageMap;
use support::StorageValue;
use support::dispatch::Result;
use support::{decl_module, decl_storage, decl_event};
use support::{ensure, fail};
use support::traits::MakePayment;
use system::{ensure_signed, ensure_root, ensure_inherent};
use balances::BalanceLock;

use support::traits::{Currency, ReservableCurrency, OnDilution, OnUnbalanced, Imbalance};
use support::traits::{LockableCurrency, LockIdentifier, WithdrawReason, WithdrawReasons};
use runtime_io::print;

#[cfg(feature = "std")]
use serde_derive::{Serialize, Deserialize};
use parity_codec::{Encode, Decode};


#[derive(Encode, Decode, Default, Clone, PartialEq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Bucket<Hash, Balance, AccountId, BlockNumber> {
	// id: AccountId,
	id: Hash,

	promise: Option<Promise<Hash, Balance, AccountId, BlockNumber>>,

	/// price for selling the bucket
	price: Balance,
}

/// Describes an accepted promise
#[derive(Encode, Decode, Default, Clone, PartialEq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Promise<Hash, Balance, AccountId, BlockNumber> {
	id: Hash,

	/// initial author of `this` promise
	/// in the near future `owner` can be removed
	/// because it exists in the global mapping
	owner: AccountId,

	/// promised value to fullfill
	value: Balance,
	/// time (number of blocks)
	period: BlockNumber,
	/// time of the end of promise
	until: Option<BlockNumber>,

	/// filled value for current period
	filled: Balance,
	/// time (in blocks) when current period was started
	acception_dt: BlockNumber,
}

/// Describes not accepted "free promise"
#[derive(Encode, Decode, Default, Clone, PartialEq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct FreePromise<Hash, Balance, /* Stake, */ BlockNumber> {
	id: Hash,
	/// promised value to fullfill
	value: Balance,
	/// time (number of blocks)
	period: BlockNumber,
	/// time of the end of promise
	until: Option<BlockNumber>,
}


// where <Self as system::Trait>::AccountId: Self::Stake::AccountId
pub trait Trait: system::Trait + balances::Trait {
	type Stake: LockableCurrency<Self::AccountId>;
	// type Stake: balances::Trait;
	// type Currency: Currency<Self::AccountId>;

	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
}


decl_event!(
	pub enum Event<T>
	where
		<T as system::Trait>::AccountId,
		<T as system::Trait>::Hash,
		<T as balances::Trait>::Balance
	{
		BucketCreated(AccountId, Hash),
		/// OwnerSet: from, to, bucket
		PriceSet(AccountId, Hash, Balance),
		Transferred(AccountId, AccountId, Hash),
		Bought(AccountId, AccountId, Hash, Balance),


		/// FreePromise is created.
		PromiseCreated(AccountId, Hash),
		/// FreePromise is changed.
		PromiseChanged(Hash),
		/// FreePromise is accepted by owner of bucket.
		/// (PromiseID:Hash, BucketID:Hash)
		PromiseAccepted(Hash, Hash),
		/// (bucket_id:Hash, promise_id:Hash, value:Balance)
		PromiseFilled(Hash, Hash, Balance),
		/// (bucket_id:Hash, promise_id:Hash)
		PromiseFullilled(Hash, Hash),
		/// (bucket_id:Hash, promise_id:Hash, missed_deposit:Balance)
		PromiseBreached(Hash, Hash, Balance),

		// Staking / Locking:
		// Issued(u16, AccountId, u64),
		Stake(Hash, AccountId, Balance),
		Withdraw(Hash, AccountId, Balance),
	}
);

decl_storage! {
	trait Store for Module<T: Trait> as C2FC {
		Buckets get(bucket): map T::Hash => Bucket<T::Hash, T::Balance, T::AccountId, T::BlockNumber>;
		BucketOwner get(owner_of): map T::Hash => Option<T::AccountId>;
		/// same as `AcceptedPromiseBucket` but by bucket_id
		BucketContributor get(contributor_of): map T::Hash => Option<T::AccountId>;

		AllBucketsArray get(bucket_by_index): map u64 => T::Hash;
		AllBucketsCount get(all_buckets_count): u64;
		AllBucketsIndex: map T::Hash => u64;

		OwnedBucketsArray get(bucket_of_owner_by_index): map (T::AccountId, u64) => T::Hash;
		OwnedBucketsCount get(owned_bucket_count): map T::AccountId => u64;
		OwnedBucketsIndex: map T::Hash => u64;


		// free promises:
		Promises get(promise): map T::Hash => FreePromise<T::Hash, T::Balance, T::BlockNumber>;
		PromiseOwner get(owner_of_promise): map T::Hash => Option<T::AccountId>;

		FreePromisesArray get(free_promise_by_index): map u64 => T::Hash;
		FreePromisesCount get(free_promises_count): u64;
		FreePromisesIndex: map T::Hash => u64;

		OwnedPromisesArray get(promise_of_owner_by_index): map (T::AccountId, u64) => T::Hash;
		OwnedPromisesCount get(owned_promise_count): map T::AccountId => u64;
		OwnedPromisesIndex: map T::Hash => u64;


		// accepted promises:
		AcceptedPromisesArray get(accepted_promise_by_index): map u64 => T::Hash;
		AcceptedPromisesCount get(accepted_promises_count): u64;
		AcceptedPromisesIndex: map T::Hash => u64;

		/// returns `bucket_id` for specified `promise_id`
		AcceptedPromiseBucket get(bucket_by_promise): map T::Hash => T::Hash;

		/// Counter total of locks
		LocksCount get(locks_count): u64;
		/// promise_id -> LockIdentifier
		LockForPromise get(lock_for_promise): map T::Hash => LockIdentifier;
		// Stake get(stake_by_promise): map T::Hash => T::Balance;

		Nonce: u64;
	}
}


decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		fn deposit_event<T>() = default;

		fn create_bucket(origin) -> Result {
			let sender = ensure_signed(origin)?;
			let nonce = <Nonce<T>>::get();
			let bucket_id = (<system::Module<T>>::random_seed(), &sender, nonce).using_encoded(<T as system::Trait>::Hashing::hash);

			let new_bucket = Bucket {
					id: bucket_id,
					promise: None,
					price: <T::Balance as As<u64>>::sa(0),
			};

			Self::mint_bucket(sender, bucket_id, new_bucket)?;

			<Nonce<T>>::mutate(|n| *n += 1);

			Ok(())
		}

		fn create_promise_until(origin, value: T::Balance, period: T::BlockNumber, until: Option<T::BlockNumber>) -> Result {
			let sender = ensure_signed(origin)?;
			let nonce = <Nonce<T>>::get();
			let promise_id = (<system::Module<T>>::random_seed(), &sender, nonce).using_encoded(<T as system::Trait>::Hashing::hash);

			let new_promise = FreePromise {
				id: promise_id,
				value,
				period,
				until,
			};

			Self::mint_promise(sender, promise_id, new_promise)?;

			<Nonce<T>>::mutate(|n| *n += 1);

			Ok(())
		}

		fn create_promise(origin, value: T::Balance, period: T::BlockNumber) -> Result {
			Self::create_promise_until(origin, value, period, None)
		}


		// fn stake_to_promise(origin, promise_id: T::Hash, amount: <T::Stake as balances::Trait>::Balance, period: <T::Stake as system::Trait>::BlockNumber) -> Result {
		// where T::BlockNumber = crate::BlockNumber
		fn stake_to_promise(origin, promise_id: T::Hash, amount: T::Balance) -> Result {
			let sender = ensure_signed(origin)?;

			ensure!(<Promises<T>>::exists(promise_id), "This promise does not exist");
			let owner = Self::owner_of_promise(promise_id).ok_or("No owner for this promise")?;
			ensure!(owner == sender, "You do not own this promise");

			// get data from existing promise:
			let until = if <AcceptedPromiseBucket<T>>::exists(promise_id) {
				let promise = {
					let bucket_id = <AcceptedPromiseBucket<T>>::get(promise_id);
					let bucket = Self::bucket(bucket_id);
					let promise = bucket.promise;
					ensure!(promise.is_some(), "Bucket doesnt contains promise");
					promise.unwrap()
				};
				promise.until
			} else {
				let promise = Self::promise(promise_id);
				promise.until
			}.unwrap_or( unsafe {
				// end of the universe:
				// TODO: use (crate::)BlockNumber::max_value()
				// <T as system::Trait>::BlockNumber::from(crate::BlockNumber::max_value())
				// <T::BlockNumber as As<crate::BlockNumber>>::sa(max as crate::BlockNumber);
				// XXX:
				let max = crate::BlockNumber::max_value();
				(*(max as *const crate::BlockNumber as *const <T as system::Trait>::BlockNumber)).clone()
			});


			let reasons = WithdrawReasons::from(WithdrawReason::Reserve);

			if <LockForPromise<T>>::exists(promise_id) {
				// let now = <system::Module<T>>::block_number();
				let lock_id = Self::lock_for_promise(promise_id);
				// select lock with specified ID:
				let lock = get_lock::<T>(&sender, &lock_id);
				let lock = { // XXX: test & remove me
					let locks_all = <balances::Module<T>>::locks(&sender);
					let mut locks = locks_all.into_iter().filter_map(|l|
						if l.id == lock_id {
							Some(l)
						} else {
							None
						});
					let lock = locks.next();
					ensure!(lock.is_none(), "Lock not found");
					ensure!(locks.next().is_some(), "Incorrect length of locks with same ID. WTF?!");
					lock.unwrap()
				};

				// TODO: check overflow:
				// ensure!(T::Balance::max_value() - lock.amount >= amount, "Overflow max size of Balance!");
				// e.g. crate::BlockNumber::max_value() - <T::Balance as As<crate::Balance>>::sa(lock.amount as crate::Balance) >= <T::Balance as As<crate::Balance>>::sa(amount as crate::Balance)

				<balances::Module<T>>::extend_lock(lock_id, &sender, lock.amount + amount, until, reasons);
			} else {
				let lock_id = Self::next_free_lock_identifier(&promise_id);

				<balances::Module<T>>::set_lock(lock_id, &sender, amount, until, reasons);

				// TODO: use T::Stake instead T::Balance:
				// <T::Stake>::set_lock(lock, &sender, amount, until, reasons);
				// <balances::Module<T::Stake>>::set_lock(lock, &sender, amount, until, reasons);

				// register new lock:
				<LockForPromise<T>>::insert(promise_id, lock_id);
				<LocksCount<T>>::mutate(|n| *n += 1);
			}

			Self::deposit_event(RawEvent::Stake(promise_id, sender, amount));

			Ok(())
		}

		fn withdraw_staken(origin, promise_id: T::Hash) -> Result {
			let sender = ensure_signed(origin)?;

			ensure!(<Promises<T>>::exists(promise_id), "This promise does not exist");

			let owner = Self::owner_of(promise_id).ok_or("No owner for this promise")?;
			ensure!(owner == sender, "You do not own this promise");

			if <LockForPromise<T>>::exists(promise_id) {
				let lock_id = Self::lock_for_promise(promise_id);

				let lock = get_lock::<T>(&sender, &lock_id);

				if let Some(lock) = &lock {
					let now = <system::Module<T>>::block_number();
					ensure!(!<AcceptedPromiseBucket<T>>::exists(promise_id), "This promise already accepted so stake cannot withdraw.");
					ensure!(lock.until <= now, "This locked balance period isn't ended and stake cannot withdraw.");
				}

				let free = {
					lock.map(|lock| lock.amount)
				}.unwrap_or(Zero::zero());

				<balances::Module<T>>::remove_lock(lock_id, &sender);

				Self::deposit_event(RawEvent::Withdraw(promise_id, sender, free));
			}

			Ok(())
		}


		fn edit_promise(origin, promise_id: T::Hash, value: T::Balance, period: T::BlockNumber) -> Result {
			let sender = ensure_signed(origin)?;

			ensure!(<Promises<T>>::exists(promise_id), "This promise does not exist");

			let owner = Self::owner_of(promise_id).ok_or("No owner for this promise")?;
			ensure!(owner == sender, "You do not own this promise");

			<Promises<T>>::mutate(promise_id, |promise|{
				promise.value = value;
				promise.period = period;
			});

			Self::deposit_event(RawEvent::PromiseChanged(promise_id));

			Ok(())
		}


		/// Accept specified free promise and add it to specified bucket.
		/// Only owner of the bucket can do it.
		fn accept_promise(origin, promise_id: T::Hash, bucket_id: T::Hash) -> Result {
			let sender = ensure_signed(origin)?;

			ensure!(<Buckets<T>>::exists(bucket_id), "This bucket does not exist");
			ensure!(<Promises<T>>::exists(promise_id), "This promise does not exist");
			ensure!(!<AcceptedPromiseBucket<T>>::exists(promise_id), "This promise is already accepted");


			let bucket_owner = Self::owner_of(bucket_id).ok_or("No owner for this bucket")?;
			ensure!(bucket_owner == sender, "You do not own this promise");

			let promise_owner = Self::owner_of_promise(promise_id).ok_or("No owner for this promise")?;
			ensure!(promise_owner != sender, "You can not accept your own promise");

			let mut bucket = Self::bucket(bucket_id);
			ensure!(bucket.promise.is_none(), "Bucket already contains another promise");

			// get current (latest) block:
			let current_block = <system::Module<T>>::block_number();

			let free_promise = Self::promise(promise_id);
			let promise = Promise {
				id: free_promise.id,
				// in the near future `owner` can be removed
				owner: promise_owner.clone(),
				value: free_promise.value,
				period: free_promise.period,
				until: free_promise.until,
				acception_dt: current_block,
				filled: <T::Balance as As<u64>>::sa(0),
			};

			bucket.promise = Some(promise);
			<Buckets<T>>::insert(bucket_id, bucket);
			<AcceptedPromiseBucket<T>>::insert(promise_id, bucket_id);

			// incrmnt the counter & push to maps:
			{
				let accepted_promises_count = Self::accepted_promises_count();
				let new_accepted_promises_count = accepted_promises_count
					.checked_add(1)
					.ok_or("Overflow adding a new promise to total supply")?;

				<BucketContributor<T>>::insert(bucket_id, promise_owner);

				<AcceptedPromisesArray<T>>::insert(accepted_promises_count, promise_id);
				<AcceptedPromisesCount<T>>::put(new_accepted_promises_count);
				<AcceptedPromisesIndex<T>>::insert(promise_id, accepted_promises_count);
			}

			<Nonce<T>>::mutate(|n| *n += 1);

			Self::deposit_event(RawEvent::PromiseAccepted(promise_id, bucket_id));

			Ok(())
		}


		// selling & trasfering a bucket //

		fn set_price(origin, bucket_id: T::Hash, new_price: T::Balance) -> Result {
			let sender = ensure_signed(origin)?;

			ensure!(<Buckets<T>>::exists(bucket_id), "This bucket does not exist");

			let owner = Self::owner_of(bucket_id).ok_or("No owner for this bucket")?;
			ensure!(owner == sender, "You do not own this bucket");

			let mut bucket = Self::bucket(bucket_id);
			bucket.price = new_price;

			<Buckets<T>>::insert(bucket_id, bucket);

			Self::deposit_event(RawEvent::PriceSet(sender, bucket_id, new_price));

			Ok(())
		}

		fn transfer(origin, to: T::AccountId, bucket_id: T::Hash) -> Result {
			let sender = ensure_signed(origin)?;

			let owner = Self::owner_of(bucket_id).ok_or("No owner for this bucket")?;
			ensure!(owner == sender, "You do not own this bucket");

			Self::transfer_from(sender, to, bucket_id)?;

			Ok(())
		}

		fn buy_bucket(origin, bucket_id: T::Hash, max_price: T::Balance) -> Result {
			let sender = ensure_signed(origin)?;

			ensure!(<Buckets<T>>::exists(bucket_id), "This bucket does not exist");

			let owner = Self::owner_of(bucket_id).ok_or("No owner for this bucket")?;
			ensure!(owner != sender, "You can't buy your own bucket");

			let mut bucket = Self::bucket(bucket_id);

			let bucket_price = bucket.price;
			ensure!(!bucket_price.is_zero(), "The bucket you want to buy is not for sale");
			ensure!(bucket_price <= max_price, "The bucket you want to buy costs more than your max price");

			Self::transfer_money(&sender, &owner, bucket_price)?;
			Self::transfer_from(owner.clone(), sender.clone(), bucket_id)?;

			bucket.price = <T::Balance as As<u64>>::sa(0);
			<Buckets<T>>::insert(bucket_id, bucket);

			Self::deposit_event(RawEvent::Bought(sender, owner, bucket_id, bucket_price));

			Ok(())
		}


		// do/fill the promises //

		fn fill_bucket(origin, bucket_id: T::Hash, deposit: T::Balance) -> Result {
			let sender = ensure_signed(origin)?;

			ensure!(<Buckets<T>>::exists(bucket_id), "This bucket does not exist");

			let owner = Self::owner_of(bucket_id).ok_or("No owner for this bucket")?;
			ensure!(owner != sender, "You can't fill your own bucket");

			let mut bucket = Self::bucket(bucket_id);
			ensure!(bucket.promise.is_some(), "This bucket does not contains promise");


			if let Some(ref mut promise) = bucket.promise {
				let promise_id = promise.id;

				ensure!(!promise.value.is_zero(), "The promise in the bucket you want to fill is invalid");
				ensure!(promise.filled <= promise.value, "The bucket you want to fill is already fullfilled");

				Self::transfer_money(&sender, &owner, deposit)?;

				promise.filled = deposit + promise.filled;

				Self::deposit_event(RawEvent::PromiseFilled(bucket_id, promise_id, deposit));

				if promise.filled >= promise.value {
					Self::deposit_event(RawEvent::PromiseFullilled(bucket_id, promise_id));
				}
			}

			// re-store the bucket
			<Buckets<T>>::insert(bucket_id, bucket);

			Ok(())
		}

		fn fullfill_bucket(origin, bucket_id: T::Hash) -> Result {
			let deposit = {
				ensure!(<Buckets<T>>::exists(bucket_id), "This bucket does not exist");
				let bucket = Self::bucket(bucket_id);
				let promise = &bucket.promise.ok_or("This bucket doesnt contains an accepted promise")?;
				let deposit = promise.filled - promise.value;
				deposit
			};

			Self::fill_bucket(origin, bucket_id, deposit)
		}



		/// Check the breach of promise at end of the each block.
		/// Simple timer here.
		fn on_finalise(n: T::BlockNumber) {
			let accepted_promises_count = Self::accepted_promises_count();

			for i in 0..accepted_promises_count {
				let promise_id = Self::accepted_promise_by_index(i);
				let bucket_id = Self::bucket_by_promise(promise_id);

				if <Buckets<T>>::exists(bucket_id) {
					let bucket = Self::bucket(bucket_id);
					// skip if bucket doesn't contains a promise
					if let Some(promise) = &bucket.promise {
						let lifetime = n - promise.acception_dt;
						let wanted_deposit = promise.filled - promise.value;
						// if (lifetime % promise.period).is_zero() && !wanted_deposit.is_zero() {
						if (lifetime % promise.period).is_zero() {
							// TODO: reset `promise.filled` to zero because new period starts.

							if wanted_deposit > <T::Balance>::zero() {
								// here we should to emit Event about *failed promise*.
								Self::deposit_event(RawEvent::PromiseBreached(bucket_id, promise_id, wanted_deposit));
								// <BucketContributor<T>>::...(bucket_id,);
							}
						}
					}
				}
			}
		}
	}
}


// private & utils //


// fn get_lock<T: Trait>(who: &T::AccountId, lock_id: &LockIdentifier) -> core::result::Result<Option<BalanceLock<T::Balance, T::BlockNumber>>, &'static str> {
// 	let lock = {
// 		let locks_all = <balances::Module<T>>::locks(who);
// 		let mut locks = locks_all.into_iter().filter_map(|l|
// 			if &l.id == lock_id {
// 				return Ok(Some(l));
// 			} else {
// 				None
// 			});
// 		let lock = locks.next();
// 		ensure!(lock.is_none(), "Lock not found");
// 		ensure!(locks.next().is_some(), "Incorrect length of locks with same ID. WTF?!");
// 		lock.unwrap()
// 	};
// 	Ok(None)
// }

fn get_lock<T: Trait>(who: &T::AccountId, lock_id: &LockIdentifier)
                      -> Option<BalanceLock<T::Balance, T::BlockNumber>> {
	let locks_all = <balances::Module<T>>::locks(who);
	let mut locks = locks_all.into_iter()
	                         .filter_map(|l| if &l.id == lock_id { Some(l) } else { None });
	locks.next()
	// ensure!(lock.is_none(), "Lock not found");
	// ensure!(locks.next().is_some(), "Incorrect length of locks with same ID. WTF?!");
}


impl<T: Trait> Module<T> {

	/// Create LockIdentifier via simple counter `locks_count`.
	/// Previously was by promise_id.
	fn next_free_lock_identifier(_promise_id: &T::Hash) -> LockIdentifier {
		// let v:&[u8] = promise_id.as_ref();
		// let a: [u8; 8] = clone_into_array(&v.as_ref()[v.len()-8..]);
		// LockIdentifier::from(a)
		use core::mem::size_of;
		let locks_count = Self::locks_count() + 1;
		let lid: [u8; size_of::<u64>()] = LockIdentifier::from(locks_count.to_le_bytes());
		lid
	}


	fn mint_bucket(to: T::AccountId, bucket_id: T::Hash,
	               new_bucket: Bucket<T::Hash, T::Balance, T::AccountId, T::BlockNumber>)
	               -> Result
	{
		ensure!(!<BucketOwner<T>>::exists(bucket_id), "Bucket already exists");

		let owned_bucket_count = Self::owned_bucket_count(&to);

		let new_owned_bucket_count = owned_bucket_count.checked_add(1)
		                                               .ok_or("Overflow adding a new bucket to account balance")?;

		let all_buckets_count = Self::all_buckets_count();

		let new_all_buckets_count = all_buckets_count.checked_add(1)
		                                             .ok_or("Overflow adding a new bucket to total supply")?;

		<Buckets<T>>::insert(bucket_id, new_bucket);
		<BucketOwner<T>>::insert(bucket_id, &to);

		<AllBucketsArray<T>>::insert(all_buckets_count, bucket_id);
		<AllBucketsCount<T>>::put(new_all_buckets_count);
		<AllBucketsIndex<T>>::insert(bucket_id, all_buckets_count);

		<OwnedBucketsArray<T>>::insert((to.clone(), owned_bucket_count), bucket_id);
		<OwnedBucketsCount<T>>::insert(&to, new_owned_bucket_count);
		<OwnedBucketsIndex<T>>::insert(bucket_id, owned_bucket_count);

		Self::deposit_event(RawEvent::BucketCreated(to, bucket_id));

		Ok(())
	}

	fn mint_promise(to: T::AccountId, promise_id: T::Hash,
	                new_promise: FreePromise<T::Hash, T::Balance, T::BlockNumber>)
	                -> Result
	{
		ensure!(!<PromiseOwner<T>>::exists(promise_id), "Promise already exists");

		let owned_promise_count = Self::owned_promise_count(&to);

		let new_owned_promise_count = owned_promise_count.checked_add(1)
		                                                 .ok_or("Overflow adding a new promise to account balance")?;

		let free_promises_count = Self::free_promises_count();

		let new_free_promises_count = free_promises_count.checked_add(1)
		                                                 .ok_or("Overflow adding a new promise to total supply")?;

		<Promises<T>>::insert(promise_id, new_promise);
		<PromiseOwner<T>>::insert(promise_id, &to);

		<FreePromisesArray<T>>::insert(free_promises_count, promise_id);
		<FreePromisesCount<T>>::put(new_free_promises_count);
		<FreePromisesIndex<T>>::insert(promise_id, free_promises_count);

		<OwnedPromisesArray<T>>::insert((to.clone(), owned_promise_count), promise_id);
		<OwnedPromisesCount<T>>::insert(&to, new_owned_promise_count);
		<OwnedPromisesIndex<T>>::insert(promise_id, owned_promise_count);

		Self::deposit_event(RawEvent::PromiseCreated(to, promise_id));

		Ok(())
	}

	fn transfer_from(from: T::AccountId, to: T::AccountId, bucket_id: T::Hash) -> Result {
		let owner = Self::owner_of(bucket_id).ok_or("No owner for this bucket")?;

		ensure!(owner == from, "'from' account does not own this bucket");

		let owned_bucket_count_from = Self::owned_bucket_count(&from);
		let owned_bucket_count_to = Self::owned_bucket_count(&to);

		let new_owned_bucket_count_to = owned_bucket_count_to.checked_add(1)
		                                                     .ok_or("Transfer causes overflow of 'to' bucket balance")?;

		let new_owned_bucket_count_from =
			owned_bucket_count_from.checked_sub(1)
			                       .ok_or("Transfer causes underflow of 'from' bucket balance")?;

		// "Swap and pop"
		let bucket_index = <OwnedBucketsIndex<T>>::get(bucket_id);
		if bucket_index != new_owned_bucket_count_from {
			let last_bucket_id = <OwnedBucketsArray<T>>::get((from.clone(), new_owned_bucket_count_from));
			<OwnedBucketsArray<T>>::insert((from.clone(), bucket_index), last_bucket_id);
			<OwnedBucketsIndex<T>>::insert(last_bucket_id, bucket_index);
		}

		<BucketOwner<T>>::insert(&bucket_id, &to);
		<OwnedBucketsIndex<T>>::insert(bucket_id, owned_bucket_count_to);

		<OwnedBucketsArray<T>>::remove((from.clone(), new_owned_bucket_count_from));
		<OwnedBucketsArray<T>>::insert((to.clone(), owned_bucket_count_to), bucket_id);

		<OwnedBucketsCount<T>>::insert(&from, new_owned_bucket_count_from);
		<OwnedBucketsCount<T>>::insert(&to, new_owned_bucket_count_to);

		Self::deposit_event(RawEvent::Transferred(from, to, bucket_id));

		Ok(())
	}

	fn transfer_money(from: &T::AccountId, to: &T::AccountId, amount: T::Balance) -> Result {
		// TODO: mig/fix legacy
		// breaking changes: https://github.com/paritytech/substrate/pull/1921
		// https://github.com/paritytech/substrate/pull/1943
		// https://github.com/paritytech/substrate/issues/2159
		// <balances::Module<T>>::make_transfer(&sender, &owner, bucket_price)?;
		// <balances::Module<T>>::transfer(origin, owner, bucket_price)?;
		// TODO: use T::Currency
		// <T::Currency>::transfer(&sender, &owner, bucket_price)?;
		// XXX: Tests needed. Possible troubles here:
		<balances::Module<T> as Currency<T::AccountId>>::transfer(&from, &to, amount)
	}


	// utilites //

	#[inline]
	pub fn is_promise_accepted(promise_id: T::Hash) -> result::Result<bool, &'static str> {
		ensure!(<Promises<T>>::exists(promise_id), "This promise does not exist");
		Ok(<AcceptedPromiseBucket<T>>::exists(promise_id))
	}
}
