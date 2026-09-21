use futures::channel::oneshot;

use super::{api, await_dispatch_preparation};

#[test]
fn cancelled_request_never_polls_dispatch_preparation() {
    futures::executor::block_on(async {
        let (sender, receiver) = oneshot::channel();
        sender.send(()).unwrap();
        let preparation = async {
            panic!("cancelled request must not prepare or contact a provider");
            #[allow(unreachable_code)]
            Ok(api::RequestParams::new_for_test(Vec::new(), Vec::new()))
        };
        assert!(
            await_dispatch_preparation(preparation, receiver)
                .await
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn pending_readiness_is_cancelled_without_opening_provider() {
    futures::executor::block_on(async {
        let (sender, receiver) = oneshot::channel();
        let preparation =
            futures::future::pending::<Result<api::RequestParams, warpui::ModelDropped>>();
        let waiting = await_dispatch_preparation(preparation, receiver);
        let cancel = async {
            sender.send(()).unwrap();
        };
        let (result, ()) = futures::join!(waiting, cancel);
        assert!(result.unwrap().is_none());
    });
}

#[test]
fn prepared_params_and_cancellation_channel_reach_dispatch_together() {
    futures::executor::block_on(async {
        let (sender, receiver) = oneshot::channel();
        let params = api::RequestParams::new_for_test(Vec::new(), Vec::new());
        let expected_model = params.model.clone();
        let (prepared, receiver) =
            await_dispatch_preparation(futures::future::ready(Ok(params)), receiver)
                .await
                .unwrap()
                .expect("prepared request");
        assert_eq!(prepared.model, expected_model);
        sender.send(()).unwrap();
        assert_eq!(receiver.await, Ok(()));
    });
}
